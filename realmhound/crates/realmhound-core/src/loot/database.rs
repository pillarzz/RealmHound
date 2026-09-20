//! SQLite database storage for loot history.
//!
//! Stores all loot drops with player state, mob attribution, and item details.
//! Provides query functions for the UI and statistics.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection, OpenFlags, Result as SqlResult, Transaction};
use std::path::{Path, PathBuf};

use super::bag_types::LootBagType;
use super::player_state::PlayerSnapshot;
use super::tracker::LootTracker;
use crate::assets::get_asset_manager;

/// Database schema version for migrations.
const SCHEMA_VERSION: i32 = 23;

/// Highest loot-history schema version this build can validate and open. Used by
/// flat-layout migration to reject databases written by a newer build.
pub const SUPPORTED_SCHEMA_VERSION: i32 = SCHEMA_VERSION;

/// Core tables every valid loot-history database must contain, independent of
/// schema version. Used by flat-layout migration to validate a backup.
pub const REQUIRED_TABLES: &[&str] = &["loot_drops", "loot_items"];

/// A stored loot drop record.
#[derive(Debug, Clone)]
pub struct LootDropRecord {
    /// Database row ID
    pub id: i64,
    /// Timestamp of the drop (Unix millis)
    pub timestamp: i64,
    /// Dungeon where drop occurred
    pub dungeon: String,
    /// Map seed (for deduplication)
    pub map_seed: i32,
    /// Character ID
    pub char_id: i32,
    /// Character name
    pub char_name: String,
    /// Character class ID
    pub class_id: i32,
    /// Character skin ID (0 for default)
    pub skin_id: i32,
    /// Clothing dye/cloth texture (tex1)
    pub tex1: u32,
    /// Accessory dye/cloth texture (tex2)
    pub tex2: u32,
    /// Character fame at time of drop
    pub fame: i32,
    /// Whether character is seasonal
    pub is_seasonal: bool,
    /// Exaltation loot bonus percentage
    pub exalt_bonus: i32,
    /// Loot drop timer was active
    pub loot_drop_active: bool,
    /// Loot tier timer was active
    pub loot_tier_active: bool,
    /// Bag type ID
    pub bag_type: i32,
    /// Mob type ID that dropped the bag
    pub mob_type: i32,
    /// Mob name (resolved from ID)
    pub mob_name: String,
    /// Items in the bag
    pub items: Vec<LootItemRecord>,
}

impl LootDropRecord {
    /// Get the display icon ID (skin if set, otherwise class).
    pub fn icon_id(&self) -> i32 {
        if self.skin_id > 0 {
            self.skin_id
        } else {
            self.class_id
        }
    }
}

/// A stored loot item record.
#[derive(Debug, Clone)]
pub struct LootItemRecord {
    /// Database row ID
    pub id: i64,
    /// Parent drop ID
    pub drop_id: i64,
    /// Slot index in bag (0-7)
    pub slot: i32,
    /// Item type ID
    pub item_id: i32,
    /// Item name (resolved from ID)
    pub item_name: String,
    /// Number of enchantments
    pub enchant_count: i32,
    /// Enchantment IDs (comma-separated for storage)
    pub enchant_ids: String,
    /// Pre-parsed enchantment IDs (computed on load)
    pub parsed_enchant_ids: Vec<i32>,
}

/// Source filters for loot history queries (DB-level filtering).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceFilters {
    /// Filter by mob type ID (None = all mobs)
    pub mob_type: Option<i32>,
    /// Cached mob name for display in UI
    pub mob_name: Option<String>,
    /// Filter by dungeon name (None = all dungeons)
    pub dungeon: Option<String>,
    /// Filter by character ID (None = all characters)
    pub char_id: Option<i32>,
    /// Cached character name for display in UI
    pub char_name: Option<String>,
    /// Cached character icon ID (skin or class) for display in UI
    pub char_icon_id: Option<i32>,
    /// Filter by item type ID (None = all items)
    pub item_id: Option<i32>,
    /// Cached item name for display in UI
    pub item_name: Option<String>,
    /// Filter by bag type IDs (None = all bag types)
    pub bag_types: Option<Vec<i32>>,
    /// Free-text query matched against mob name, dungeon, or item name
    /// (case-insensitive LIKE). None/empty = no text filter.
    pub search_text: Option<String>,
}

impl SourceFilters {
    /// Check if any source filter is active (excluding bag types which are always set).
    pub fn is_empty(&self) -> bool {
        self.mob_type.is_none()
            && self.dungeon.is_none()
            && self.char_id.is_none()
            && self.item_id.is_none()
            && self
                .search_text
                .as_ref()
                .map(|s| s.trim().is_empty())
                .unwrap_or(true)
    }

    /// Clear all source filters (preserves bag_types).
    pub fn clear(&mut self) {
        let bag_types = self.bag_types.take();
        *self = Self::default();
        self.bag_types = bag_types;
    }

    /// Set bag type filter.
    pub fn set_bag_types(&mut self, bag_types: Vec<i32>) {
        self.bag_types = Some(bag_types);
    }

    /// Set mob filter.
    pub fn set_mob(&mut self, mob_type: i32, mob_name: String) {
        self.mob_type = Some(mob_type);
        self.mob_name = Some(mob_name);
    }

    /// Clear mob filter.
    pub fn clear_mob(&mut self) {
        self.mob_type = None;
        self.mob_name = None;
    }

    /// Set dungeon filter.
    pub fn set_dungeon(&mut self, dungeon: String) {
        self.dungeon = Some(dungeon);
    }

    /// Clear dungeon filter.
    pub fn clear_dungeon(&mut self) {
        self.dungeon = None;
    }

    /// Set character filter.
    pub fn set_char(&mut self, char_id: i32, char_name: String, icon_id: i32) {
        self.char_id = Some(char_id);
        self.char_name = Some(char_name);
        self.char_icon_id = Some(icon_id);
    }

    /// Clear character filter.
    pub fn clear_char(&mut self) {
        self.char_id = None;
        self.char_name = None;
        self.char_icon_id = None;
    }

    /// Set item filter.
    pub fn set_item(&mut self, item_id: i32, item_name: String) {
        self.item_id = Some(item_id);
        self.item_name = Some(item_name);
    }

    /// Clear item filter.
    pub fn clear_item(&mut self) {
        self.item_id = None;
        self.item_name = None;
    }
}

/// Input for inserting a new loot drop.
#[derive(Debug, Clone)]
pub struct NewLootDrop {
    /// Timestamp of the drop (Unix millis)
    pub timestamp: i64,
    /// Player snapshot at time of drop
    pub player: PlayerSnapshot,
    /// Bag type
    pub bag_type: LootBagType,
    /// Mob type ID
    pub mob_type: i32,
    /// Mob name
    pub mob_name: String,
    /// Items in the bag
    pub items: Vec<NewLootItem>,
}

/// Input for a new loot item.
#[derive(Debug, Clone)]
pub struct NewLootItem {
    /// Slot index (0-7)
    pub slot: i32,
    /// Item type ID
    pub item_id: i32,
    /// Item name
    pub item_name: String,
    /// Enchantment IDs
    pub enchant_ids: Vec<i32>,
}

/// Loot database manager.
pub struct LootDatabase {
    conn: Connection,
    /// Paired combat-history path used by migration backfills that correlate
    /// Unknown loot sources with recorded boss kills. Supplied before
    /// `initialize` because the v13 backfill reads it. `None` disables that
    /// correlation (in-memory and offline-tooling databases).
    combat_path: Option<PathBuf>,
}

impl LootDatabase {
    /// Open or create the loot database (writer connection) at an explicit path.
    ///
    /// Enables WAL journaling and a busy timeout so a concurrent read-only
    /// connection (the UI's `LootPanel`) can read while the worker thread writes.
    /// Creates/migrates tables if needed. `combat_path` is the paired combat
    /// history database used by the v13 Unknown-source backfill; it is stored
    /// before migration runs.
    pub fn open_writer(loot_path: &Path, combat_path: Option<&Path>) -> SqlResult<Self> {
        if let Some(parent) = loot_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(loot_path)?;
        Self::apply_concurrency_pragmas(&conn)?;
        let mut db = Self {
            conn,
            combat_path: combat_path.map(Path::to_path_buf),
        };
        db.initialize()?;
        Ok(db)
    }

    /// Open a read-only connection to the loot database at an explicit path.
    ///
    /// Used by the UI to read loot history while the worker thread owns the
    /// single writer connection. Opens with `SQLITE_OPEN_READ_ONLY` and sets a
    /// busy timeout but does NOT create or migrate the schema, nor set the
    /// journal mode (the writer owns WAL + schema). The writer must have been
    /// opened first so the file, tables and `-wal`/`-shm` exist.
    pub fn open_reader_at(loot_path: &Path) -> SqlResult<Self> {
        let conn = Connection::open_with_flags(
            loot_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(Self {
            conn,
            combat_path: None,
        })
    }

    /// Enable WAL journaling + a busy timeout for safe concurrent reader/writer
    /// access across threads (each on its own connection).
    fn apply_concurrency_pragmas(conn: &Connection) -> SqlResult<()> {
        // WAL lets a reader and a writer operate concurrently on separate
        // connections; busy_timeout makes either side wait out a transient lock
        // instead of failing with SQLITE_BUSY.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let _: String = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
        Ok(())
    }

    /// Open an in-memory database (for testing).
    pub fn open_in_memory() -> SqlResult<Self> {
        let conn = Connection::open_in_memory()?;
        let mut db = Self {
            conn,
            combat_path: None,
        };
        db.initialize()?;
        Ok(db)
    }

    /// Checkpoint the WAL and close, so a following relaunch leaves no `-wal` tail.
    pub fn close(self) -> SqlResult<()> {
        let _: (i64, i64, i64) =
            self.conn
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?;
        self.conn.close().map_err(|(_, err)| err)
    }

    /// Initialize the database schema.
    fn initialize(&mut self) -> SqlResult<()> {
        // Check schema version
        let version = self.get_schema_version();

        if version == 0 {
            // Fresh database - create tables
            self.create_tables()?;
            self.set_schema_version(SCHEMA_VERSION)?;
        } else if version < SCHEMA_VERSION {
            // Need migration (future use)
            self.migrate(version)?;
        }

        Ok(())
    }

    /// Get current schema version.
    fn get_schema_version(&self) -> i32 {
        self.conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or(0)
    }

    /// Set schema version.
    fn set_schema_version(&self, version: i32) -> SqlResult<()> {
        self.conn
            .execute(&format!("PRAGMA user_version = {}", version), [])?;
        Ok(())
    }

    /// Create database tables.
    fn create_tables(&self) -> SqlResult<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS loot_drops (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                dungeon TEXT NOT NULL,
                map_seed INTEGER NOT NULL,
                char_id INTEGER NOT NULL,
                char_name TEXT NOT NULL,
                class_id INTEGER NOT NULL,
                skin_id INTEGER NOT NULL DEFAULT 0,
                tex1 INTEGER NOT NULL DEFAULT 0,
                tex2 INTEGER NOT NULL DEFAULT 0,
                fame INTEGER NOT NULL,
                is_seasonal INTEGER NOT NULL DEFAULT 0,
                exalt_bonus INTEGER NOT NULL DEFAULT 0,
                loot_drop_active INTEGER NOT NULL DEFAULT 0,
                loot_tier_active INTEGER NOT NULL DEFAULT 0,
                bag_type INTEGER NOT NULL,
                mob_type INTEGER NOT NULL,
                mob_name TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS loot_items (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                drop_id INTEGER NOT NULL,
                slot INTEGER NOT NULL,
                item_id INTEGER NOT NULL,
                item_name TEXT NOT NULL,
                enchant_count INTEGER NOT NULL DEFAULT 0,
                enchant_ids TEXT NOT NULL DEFAULT '',
                FOREIGN KEY (drop_id) REFERENCES loot_drops(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_drops_timestamp ON loot_drops(timestamp DESC);
            CREATE INDEX IF NOT EXISTS idx_drops_dungeon ON loot_drops(dungeon);
            CREATE INDEX IF NOT EXISTS idx_drops_bag_type ON loot_drops(bag_type);
            CREATE INDEX IF NOT EXISTS idx_drops_char_id ON loot_drops(char_id);
            CREATE INDEX IF NOT EXISTS idx_drops_mob_type ON loot_drops(mob_type);
            CREATE INDEX IF NOT EXISTS idx_items_drop_id ON loot_items(drop_id);
            "#,
        )
    }

    /// Run migrations from old version to current.
    fn migrate(&mut self, from_version: i32) -> SqlResult<()> {
        // Migration from v1 to v2: add skin_id column
        if from_version < 2 {
            self.conn.execute(
                "ALTER TABLE loot_drops ADD COLUMN skin_id INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        // Migration from v2 to v3: add tex1/tex2 columns for character dyes
        if from_version < 3 {
            self.conn.execute(
                "ALTER TABLE loot_drops ADD COLUMN tex1 INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
            self.conn.execute(
                "ALTER TABLE loot_drops ADD COLUMN tex2 INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        // Migration from v3 to v4: backfill Janus attribution. Bags containing
        // Mark of Janus are genuine Janus drops but were stored as Unknown
        // (mob_type 0) or misattributed to a nearby mob, since Janus emits loot
        // anonymously. Reclassify them so they display and filter as Janus.
        if from_version < 4 {
            self.conn.execute(
                &format!(
                    "UPDATE loot_drops SET mob_type = {}, mob_name = '{}'
                     WHERE mob_type != {} AND id IN (
                         SELECT drop_id FROM loot_items WHERE item_id = {}
                     )",
                    super::JANUS_OBJECT_TYPE,
                    super::JANUS_NAME,
                    super::JANUS_OBJECT_TYPE,
                    super::MARK_OF_JANUS_ITEM_ID,
                ),
                [],
            )?;
        }
        // Migration from v4 to v5: backfill Lair of Draconis dragon attribution.
        // The four elemental dragons transform on death and emit loot
        // anonymously, so their bags were stored as Unknown. Reclassify each
        // Unknown LoD bag to the dragon identified by its item signature.
        if from_version < 5 {
            self.backfill_unknown_by_item_signal(
                super::LAIR_OF_DRACONIS_NAME,
                false,
                super::lair_of_draconis_dragon,
            )?;
        }
        // Migration from v5 to v6: backfill Spectral Penitentiary attribution.
        // Minibosses (e.g. Oculon) transform/despawn on death, so their bags
        // were stored as Unknown. Reclassify each Unknown, non-brown SpecPen bag
        // to the boss its items identify, validating the (weaker) potion signal
        // against the dungeon's rules (at most two minibosses per run).
        if from_version < 6 {
            self.backfill_spectral_penitentiary()?;
        }
        // Migration from v6 to v7: backfill boss attribution across all dungeons
        // for any remaining Unknown, non-brown bag whose items are a unique boss
        // drop (per the RealmEye drop tables). Bags whose items could have come
        // from more than one boss are left Unknown (disambiguation needs combat
        // data, which a one-time loot migration cannot reliably replay).
        if from_version < 7 {
            self.backfill_unique_boss_drops()?;
        }
        // Migration from v7 to v8: backfill Tomb of the Ancients attribution.
        // The three main bosses (Bes/Geb/Nut) sit in separate rooms and usually
        // die offscreen, so their bags were stored Unknown; and Life bags near an
        // artifact were mis-attributed to it (artifacts never drop Life). Applies
        // the live rules to history, per run: Mark of Geb -> Geb, unique drop
        // signatures, strip Life credited to a non-Life-dropper, then assign the
        // remaining Unknown bags to still-un-credited bosses alphabetically (one
        // per boss per run). Liveness (which boss was on-screen) cannot be
        // replayed from stored data, so every main boss is treated as a
        // candidate.
        if from_version < 8 {
            self.backfill_tomb_of_the_ancients()?;
        }
        // Migration from v8 to v9: re-run the Tomb backfill. The v8 pass wrongly
        // let a Mark of Geb bag consume Geb's slot in the alphabetical fallback,
        // leaving a Life bag that belonged to Geb stored as Unknown. The Mark is
        // a separate invisible-spawner bag, not one of Geb's loot bags, so it no
        // longer consumes a slot. The backfill is re-run-safe: bags already
        // attributed to a main boss seed the run and are left unchanged, so only
        // still-Unknown bags are (re)resolved.
        if from_version < 9 {
            self.backfill_tomb_of_the_ancients()?;
        }
        // Migration from v9 to v10: re-run the Tomb backfill once more. The
        // Mark of Geb and Potion of Life signals were previously resolved by
        // object display name, which returned the invisible Mark *spawner*
        // object instead of the item, so both rules silently never fired: Mark
        // bags kept a proximity guess (often Nut) and seeded Geb's slot, and
        // Life mis-attribution went undetected. Now that both use hardcoded item
        // ids, re-running corrects those bags. Still re-run-safe (main-boss bags
        // seed the run and are left unchanged; only Mark, Life-on-artifact and
        // Unknown bags are (re)resolved).
        if from_version < 10 {
            self.backfill_tomb_of_the_ancients()?;
        }
        // Migration from v10 to v11: re-run the Tomb backfill after teaching it to
        // cap each main boss at one real bag. The earlier passes left bags that
        // were already on a main boss untouched, so a boss that live proximity
        // credited with two Life bags kept both while another main boss (which the
        // Mark of Geb proves also died) got none. The reworked backfill now
        // redistributes the extra bag to the un-credited boss.
        if from_version < 11 {
            self.backfill_tomb_of_the_ancients()?;
        }
        // Migration from v11 to v12: re-run the Tomb backfill after teaching it to
        // recognize "modifier spawner" bags (pet food, eggs, skin tokens,
        // effusions, mystery-stat crates). Such a bag holds no boss loot but
        // proximity could pin it to a main boss, stealing that boss's one-bag slot
        // and leaving the boss's real bag Unknown. The backfill now credits these
        // to Geb without a slot, freeing the stolen slot for the real bag.
        if from_version < 12 {
            self.backfill_tomb_of_the_ancients()?;
        }
        // Migration from v12 to v13: backfill still-Unknown loot sources ('?')
        // from recorded Combat History fights. Earlier passes could only use item
        // signatures and left bags that need a confirmed kill to disambiguate (a
        // boss that died offscreen and emits loot anonymously) as Unknown. Now
        // that fights are persisted with map_seed + boss identity, correlate each
        // remaining Unknown bag with a boss killed in the same instance, applying
        // the same rules the live tracker now uses. If the combat history exists
        // but cannot be read this launch (e.g. mid-migration or locked), defer:
        // leave the schema at v12 so the backfill retries next launch rather than
        // being permanently skipped.
        if from_version < 13 {
            if !self.backfill_unknown_from_combat_fights()? {
                self.set_schema_version(12)?;
                return Ok(());
            }
        }
        // Migration from v13 to v14: re-run the Tomb backfill after the live
        // tracker learned that the Mark of Geb confirms every main boss dead
        // (it only spawns once all three are defeated). A boss that ran offscreen
        // while still inside its on-screen "alive" window used to be excluded as
        // a candidate, so its real bag was stored Unknown. The distribution pass
        // hands each such orphan bag to the still-un-credited main boss, so those
        // legacy '?' bags now resolve. Re-run-safe (main-boss bags seed the run
        // and are left unchanged; only Unknown/mis-attributed bags re-resolve).
        if from_version < 14 {
            self.backfill_tomb_of_the_ancients()?;
        }
        // Migration from v14 to v15: backfill Ice Tomb attribution. Each boss
        // (Frimar/Polaris/Glacius) drops its loot via a soul the player never
        // hits, so every Ice Tomb bag was stored Unknown. The live tracker now
        // whitelists the souls as droppers; this applies the same rules to
        // history per run: a unique per-boss ring names its boss, then remaining
        // Unknown bags that hold boss loot are handed to a still-un-credited
        // boss (alphabetically -- death order cannot be replayed from history),
        // one per boss per run. Bags already on an Ice Tomb boss seed the run and
        // are left unchanged, so the pass is re-run-safe.
        if from_version < 15 {
            self.backfill_ice_tomb()?;
        }
        // Migration from v15 to v16: earlier builds stored each Ice Tomb bag's
        // soul id as its `mob_type`, which drew the soul sprite instead of the
        // boss. Remap those soul ids onto the real boss object ids so the loot
        // history renders the boss (the boss name was already correct). No-op on
        // databases first migrated by the current build (which store boss ids).
        if from_version < 16 {
            self.remap_ice_tomb_soul_ids()?;
        }
        // Migration from v16 to v17: some bags were attributed to environmental
        // structures (walls/gates/pillars/room-checks) that registered hits via
        // EnemyOccupySquare and were removed on gate-unlock, stealing the drop
        // from the real nearby enemy. Relabel any stored source the catalog
        // positively identifies as a non-valid drop source to Unknown ("?"),
        // keeping the drop itself. Types unknown to the catalog are left as-is.
        if from_version < 17 {
            self.relabel_invalid_drop_sources()?;
        }
        // Migration from v17 to v18: Chief Beisa's oversized Oryx's Sanctuary
        // room means he routinely dies offscreen, so his high-tier bags were
        // stored Unknown. Most of these runs predate Combat History, so there is
        // no fight to correlate; instead attribute Unknown non-brown O3 bags that
        // fit Beisa's drop profile (Greater Life/Mana, red bag + T13, or a Beisa
        // drop-table item) to him. Bags that don't fit stay Unknown.
        if from_version < 18 {
            self.attribute_oryx_sanctuary_beisa()?;
        }
        // Migration from v18 to v19: an intermediate build could consume v18
        // before the Beisa relabel body shipped, leaving history stuck Unknown
        // with no retry. Re-run the (idempotent) attribution so those installs
        // are repaired; it only touches still-Unknown O3 bags fitting Beisa's
        // profile, so a DB already fixed at v18 is a no-op.
        if from_version < 19 {
            self.attribute_oryx_sanctuary_beisa()?;
        }
        // Migration from v19 to v20: backfill Bradley the Barkeep attribution.
        // The Tavern's final boss never drops loot directly (his bags are emitted
        // anonymously and were stored as Unknown or misattributed to a nearby
        // mob), but Mark of the Barkeep is a guaranteed, exclusive drop. Reclassify
        // any bag holding the Mark to Bradley the Barkeep.
        if from_version < 20 {
            self.conn.execute(
                &format!(
                    "UPDATE loot_drops SET mob_type = {}, mob_name = '{}'
                     WHERE mob_type != {} AND id IN (
                         SELECT drop_id FROM loot_items WHERE item_id = {}
                     )",
                    super::TAVERN_BARKEEP_OBJECT_TYPE,
                    super::TAVERN_BARKEEP_NAME,
                    super::TAVERN_BARKEEP_OBJECT_TYPE,
                    super::MARK_OF_THE_BARKEEP_ITEM_ID,
                ),
                [],
            )?;
        }
        // Migration from v20 to v21: backfill Killer Bee Nest realm event
        // attribution. The event's loot is emitted by the invisible EH Event
        // Taunt Controller, so every bag was stored as that controller (or
        // Unknown) instead of the Beehemoth that dropped it. Reclassify each
        // non-brown Realm bag currently on the taunt controller / Unknown by the
        // colour of its Beehemoth loot: a single colour -> that Beehemoth, mixed
        // colours -> Corrupted Bramblethorn (the legacy default; no Keyper event
        // predates this), a colourless taunt-controller bag -> Blue Beehemoth.
        // Colourless Unknown bags are left as-is (not necessarily event bags).
        if from_version < 21 {
            self.backfill_killer_bee_nest()?;
        }
        // Migration from v21 to v22: reduce Unknown bags by their rare loot's
        // known drop source. (a) Bags whose rare loot points to a single realm
        // event or single dungeon boss are reattributed to that source.
        // (b) Bags holding a boss completion Mark are reattributed to that
        // dungeon's main boss, resolved by per-dungeon consensus from bags
        // already attributed to a boss. Both skip curated dungeons (which have
        // their own logic) and are idempotent (only touch Unknown bags).
        if from_version < 22 {
            self.backfill_unknown_by_item_source()?;
            self.backfill_marks_by_dungeon_consensus()?;
        }
        // Migration from v22 to v23: re-run the Combat History fight-correlation
        // backfill. The Killer Bee Nest resolver now attributes a colourless bag
        // that arrived Unknown to the Beehemoth killed closest to it in the same
        // instance (the invisible EH dropper is never hit, so proximity can't tag
        // these bags). The v13 pass predates this rule, so legacy Beehemoth event
        // bags stayed '?'; replaying it repairs them. Idempotent (only touches
        // still-Unknown bags) and defers if the combat DB can't be read.
        if from_version < 23 {
            if !self.backfill_unknown_from_combat_fights()? {
                self.set_schema_version(22)?;
                return Ok(());
            }
        }
        self.set_schema_version(SCHEMA_VERSION)?;
        Ok(())
    }

    /// Reattribute legacy Killer Bee Nest event bags from the invisible taunt
    /// controller / Unknown to the Beehemoth (or Bramblethorn/Keyper) that
    /// dropped them, keyed off the colour of the loot the bag holds. Uses the
    /// live rules ([`LootTracker::resolve_killer_bee_nest`]) with no recorded
    /// kills, yielding the legacy defaults (Corrupted Bramblethorn for mixed
    /// colours, Blue for a colourless event bag). Idempotent: only bags on
    /// `mob_type` 0 or the taunt controller are considered, and the resolver
    /// returns `None` (no update) for colourless Unknown bags.
    fn backfill_killer_bee_nest(&mut self) -> SqlResult<()> {
        struct Bag {
            id: i64,
            map_seed: i32,
            timestamp: i64,
            bag_type: i32,
            mob_type: i32,
            items: Vec<i32>,
        }

        let mut bags: Vec<Bag> = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT d.id, d.map_seed, d.timestamp, d.bag_type, d.mob_type, i.item_id
                 FROM loot_drops d
                 LEFT JOIN loot_items i ON i.drop_id = d.id
                 WHERE (TRIM(d.dungeon) = '' OR d.dungeon LIKE '{%' OR d.dungeon = ?1)
                   AND d.mob_type IN (0, ?2)
                 ORDER BY d.id",
            )?;
            let rows = stmt.query_map(
                params![
                    super::REALM_NAME,
                    super::EH_EVENT_TAUNT_CONTROLLER_OBJECT_TYPE
                ],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i32>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i32>(3)?,
                        r.get::<_, i32>(4)?,
                        r.get::<_, Option<i32>>(5)?,
                    ))
                },
            )?;
            for row in rows {
                let (id, map_seed, timestamp, bag_type, mob_type, item) = row?;
                match bags.last_mut() {
                    Some(b) if b.id == id => {
                        if let Some(it) = item {
                            b.items.push(it);
                        }
                    }
                    _ => {
                        let mut items = Vec::new();
                        if let Some(it) = item {
                            items.push(it);
                        }
                        bags.push(Bag {
                            id,
                            map_seed,
                            timestamp,
                            bag_type,
                            mob_type,
                            items,
                        });
                    }
                }
            }
        }

        let mut updates: Vec<(i64, i32, String)> = Vec::new();
        for b in &bags {
            let Some(bag_type) = LootBagType::from_id(b.bag_type) else {
                continue;
            };
            // Brown (public/player) bags are never boss drops.
            if matches!(bag_type, LootBagType::Brown | LootBagType::BoostedBrown) {
                continue;
            }
            if let Some((t, n)) = LootTracker::resolve_killer_bee_nest(
                &[],
                b.mob_type,
                &b.items,
                b.map_seed,
                b.timestamp,
            ) {
                updates.push((b.id, t, n));
            }
        }
        if updates.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        for (id, t, n) in updates {
            tx.execute(
                "UPDATE loot_drops SET mob_type = ?1, mob_name = ?2 WHERE id = ?3",
                params![t, n, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Reduce Unknown bags by their rare loot's known drop source. Replays the
    /// live attribution ([`LootTracker::resolve_boss_override_with`]) with no
    /// recorded kills over every Unknown (`mob_type` 0), non-brown bag, so a bag
    /// whose loot points to a single realm event (e.g. the Ethereal Shrine) or a
    /// single dungeon boss is reattributed to that source. Ambiguous loot yields
    /// no change. Boss Marks need a recorded kill (absent here) and are handled
    /// by [`Self::backfill_marks_by_dungeon_consensus`] instead. Idempotent: only
    /// touches `mob_type` 0 rows, never overriding an existing attribution.
    fn backfill_unknown_by_item_source(&mut self) -> SqlResult<()> {
        struct Bag {
            id: i64,
            dungeon: String,
            bag_type: i32,
            items: Vec<i32>,
        }

        let mut bags: Vec<Bag> = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT d.id, d.dungeon, d.bag_type, i.item_id
                 FROM loot_drops d
                 LEFT JOIN loot_items i ON i.drop_id = d.id
                 WHERE d.mob_type = 0
                 ORDER BY d.id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i32>(2)?,
                    r.get::<_, Option<i32>>(3)?,
                ))
            })?;
            for row in rows {
                let (id, dungeon, bag_type, item) = row?;
                match bags.last_mut() {
                    Some(b) if b.id == id => {
                        if let Some(it) = item {
                            b.items.push(it);
                        }
                    }
                    _ => {
                        let mut items = Vec::new();
                        if let Some(it) = item {
                            items.push(it);
                        }
                        bags.push(Bag {
                            id,
                            dungeon,
                            bag_type,
                            items,
                        });
                    }
                }
            }
        }

        let mut updates: Vec<(i64, i32, String)> = Vec::new();
        for b in &bags {
            let Some(bag_type) = LootBagType::from_id(b.bag_type) else {
                continue;
            };
            if matches!(bag_type, LootBagType::Brown | LootBagType::BoostedBrown) {
                continue;
            }
            // Curated dungeons carry their own precise attribution logic (and
            // dedicated migrations); never override their decisions here.
            if super::has_curated_boss_attribution(&b.dungeon) {
                continue;
            }
            let hit = if super::is_realm_dungeon(&b.dungeon) {
                // Open-realm event loot pointing to a single non-seasonal event.
                super::resolve_realm_event_source(&b.items)
            } else {
                // Named dungeon: attribute only when the loot's drop table names
                // exactly one boss of this dungeon (unambiguous, no kill needed).
                let cands = super::boss_drop_candidates(&b.dungeon, &b.items);
                match cands.as_slice() {
                    [only] => Some(only.clone()),
                    _ => None,
                }
            };
            if let Some((t, n)) = hit {
                updates.push((b.id, t, n));
            }
        }
        if updates.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        for (id, t, n) in updates {
            tx.execute(
                "UPDATE loot_drops SET mob_type = ?1, mob_name = ?2 WHERE id = ?3",
                params![t, n, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Attribute legacy Unknown bags holding a boss completion Mark to that
    /// dungeon's main boss. A Mark is a guaranteed, exclusive main-boss drop, but
    /// the same Mark maps to different bosses across a dungeon's variants (e.g.
    /// Mark of Malphas: Archdemon Malphas vs Malphas, Gilded Forgemaster), so the
    /// correct target is the main boss of the *bag's* dungeon. These legacy bags
    /// predate combat tracking (no recorded kill), so the boss is resolved by
    /// consensus: per `(dungeon, mark)`, the most common boss already attributed
    /// to bags carrying that Mark in that dungeon, provided it is boss-like and a
    /// valid drop source. Skips curated dungeons (own Mark logic) and applies
    /// only when a clear winner exists. Idempotent (`mob_type` 0 only).
    fn backfill_marks_by_dungeon_consensus(&mut self) -> SqlResult<()> {
        let assets = get_asset_manager();
        let marks = assets.boss_mark_item_ids();
        if marks.is_empty() {
            return Ok(());
        }

        struct Bag {
            id: i64,
            dungeon: String,
            mob_type: i32,
            mob_name: String,
            items: Vec<i32>,
        }

        // Load every bag that carries a boss Mark, both attributed (for the
        // consensus vote) and Unknown (to be fixed).
        let mut bags: Vec<Bag> = Vec::new();
        {
            let mark_list = marks
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT d.id, d.dungeon, d.mob_type, d.mob_name, i.item_id
                 FROM loot_drops d
                 JOIN loot_items i ON i.drop_id = d.id
                 WHERE d.id IN (SELECT drop_id FROM loot_items WHERE item_id IN ({mark_list}))
                 ORDER BY d.id"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i32>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i32>(4)?,
                ))
            })?;
            for row in rows {
                let (id, dungeon, mob_type, mob_name, item) = row?;
                match bags.last_mut() {
                    Some(b) if b.id == id => b.items.push(item),
                    _ => bags.push(Bag {
                        id,
                        dungeon,
                        mob_type,
                        mob_name,
                        items: vec![item],
                    }),
                }
            }
        }

        // Keep only bags from non-curated dungeons (curated dungeons resolve
        // their own Marks); the SQL already guaranteed each carries a Mark.
        bags.retain(|b| !super::has_curated_boss_attribution(&b.dungeon));

        // Build per-(dungeon, mark) vote tallies from already-attributed bags,
        // keeping only boss-like, valid drop sources to filter minion noise.
        use std::collections::HashMap;
        let mut votes: HashMap<(String, i32), HashMap<(i32, String), u32>> = HashMap::new();
        for b in &bags {
            if b.mob_type == 0 {
                continue;
            }
            let boss_like = assets.is_boss_like(b.mob_type) || assets.is_curated_boss(b.mob_type);
            if !boss_like || !assets.is_valid_drop_source(b.mob_type) {
                continue;
            }
            for it in &b.items {
                if marks.contains(it) {
                    *votes
                        .entry((b.dungeon.clone(), *it))
                        .or_default()
                        .entry((b.mob_type, b.mob_name.clone()))
                        .or_insert(0) += 1;
                }
            }
        }

        // Resolve the winning boss per (dungeon, mark): a unique mode.
        let winner = |dungeon: &str, mark: i32| -> Option<(i32, String)> {
            let tally = votes.get(&(dungeon.to_string(), mark))?;
            let mut best: Option<(&(i32, String), u32)> = None;
            let mut tie = false;
            for (k, &c) in tally {
                match best {
                    Some((_, bc)) if c > bc => {
                        best = Some((k, c));
                        tie = false;
                    }
                    Some((_, bc)) if c == bc => tie = true,
                    None => best = Some((k, c)),
                    _ => {}
                }
            }
            match best {
                Some((k, _)) if !tie => Some(k.clone()),
                _ => None,
            }
        };

        let mut updates: Vec<(i64, i32, String)> = Vec::new();
        for b in &bags {
            if b.mob_type != 0 {
                continue;
            }
            let Some(mark) = b.items.iter().copied().find(|it| marks.contains(it)) else {
                continue;
            };
            if let Some((t, n)) = winner(&b.dungeon, mark) {
                updates.push((b.id, t, n));
            }
        }
        if updates.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        for (id, t, n) in updates {
            tx.execute(
                "UPDATE loot_drops SET mob_type = ?1, mob_name = ?2 WHERE id = ?3",
                params![t, n, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// combat-correlated backfill, this needs no recorded fight: Beisa is the one
    /// O3 miniboss that reliably dies offscreen (the others die onscreen and are
    /// attributed at death), so an Unknown non-brown O3 bag fitting his profile
    /// ([`bag_matches_beisa_profile`](super::bag_matches_beisa_profile)) is his.
    /// Idempotent; a no-op when assets are unavailable and the bag holds no
    /// whitelisted potion. Bags that don't fit the profile are left Unknown.
    fn attribute_oryx_sanctuary_beisa(&mut self) -> SqlResult<()> {
        struct Bag {
            id: i64,
            bag_type: i32,
            items: Vec<i32>,
        }

        let mut bags: Vec<Bag> = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT d.id, d.bag_type, i.item_id
                 FROM loot_drops d
                 LEFT JOIN loot_items i ON i.drop_id = d.id
                 WHERE d.mob_type = 0 AND d.dungeon = ?1
                 ORDER BY d.id",
            )?;
            let rows = stmt.query_map(params![super::ORYX_SANCTUARY_NAME], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i32>(1)?,
                    r.get::<_, Option<i32>>(2)?,
                ))
            })?;
            for row in rows {
                let (id, bag_type, item) = row?;
                match bags.last_mut() {
                    Some(b) if b.id == id => {
                        if let Some(it) = item {
                            b.items.push(it);
                        }
                    }
                    _ => {
                        let mut items = Vec::new();
                        if let Some(it) = item {
                            items.push(it);
                        }
                        bags.push(Bag {
                            id,
                            bag_type,
                            items,
                        });
                    }
                }
            }
        }

        let mut ids: Vec<i64> = Vec::new();
        for b in &bags {
            let Some(bag_type) = LootBagType::from_id(b.bag_type) else {
                continue;
            };
            // Brown (public/player) bags are never attributed to a mob.
            if matches!(bag_type, LootBagType::Brown | LootBagType::BoostedBrown) {
                continue;
            }
            if super::bag_matches_beisa_profile(bag_type, &b.items) {
                ids.push(b.id);
            }
        }
        if ids.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        for id in ids {
            tx.execute(
                "UPDATE loot_drops SET mob_type = ?1, mob_name = ?2 WHERE id = ?3",
                params![super::CHIEF_BEISA_OBJECT_TYPE, super::CHIEF_BEISA_NAME, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Relabel loot drops attributed to non-valid drop sources (environmental
    /// structures) to Unknown, keeping the drop rows. Idempotent: only rows
    /// whose `mob_type` is positively classified invalid by the catalog are
    /// touched, so it is safe to re-run and a no-op when assets are unavailable.
    fn relabel_invalid_drop_sources(&mut self) -> SqlResult<()> {
        let mob_types: Vec<i32> = {
            let mut stmt = self
                .conn
                .prepare("SELECT DISTINCT mob_type FROM loot_drops WHERE mob_type != 0")?;
            let it = stmt.query_map([], |r| r.get::<_, i32>(0))?;
            it.collect::<SqlResult<Vec<_>>>()?
        };
        if mob_types.is_empty() {
            return Ok(());
        }
        let assets = get_asset_manager();
        // Only relabel a type the catalog knows AND classifies as invalid; leave
        // types unknown to the catalog (e.g. assets not yet downloaded) untouched.
        let invalid: Vec<i32> = mob_types
            .into_iter()
            .filter(|&t| assets.object_class(t).is_some() && !assets.is_valid_drop_source(t))
            .collect();
        if invalid.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        for otype in invalid {
            tx.execute(
                "UPDATE loot_drops SET mob_type = 0, mob_name = '?' WHERE mob_type = ?1",
                params![otype],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Remap Ice Tomb bags stored with a soul object id onto the real boss
    /// object id, so the loot history draws the boss sprite. Idempotent.
    fn remap_ice_tomb_soul_ids(&mut self) -> SqlResult<()> {
        for boss in super::ICE_TOMB_BOSSES {
            let soul = super::ice_tomb_soul_for_boss(boss);
            let object = super::ice_tomb_object_type_for_boss(boss);
            if soul == 0 || object == 0 {
                continue;
            }
            self.conn.execute(
                "UPDATE loot_drops SET mob_type = ?1, mob_name = ?2
                 WHERE dungeon = ?3 AND mob_type = ?4",
                params![object, boss, super::ICE_TOMB_NAME, soul],
            )?;
        }
        Ok(())
    }

    /// Backfill Tomb of the Ancients boss attribution for legacy bags, mirroring
    /// the live [`resolve_tomb_attribution`](super::tracker) rules on stored
    /// history. Bags are grouped by run (`map_seed`) and processed in two passes:
    /// a deterministic pass (Mark of Geb, unique drop signatures, Sarcophagus
    /// left untouched) records which main bosses genuinely hold a bag, then a
    /// distribution pass hands the remaining ambiguous Life/junk bags out one per
    /// main boss. This caps each of Bes/Geb/Nut at a single real bag, so a boss
    /// that proximity credited with two bags no longer keeps both while another
    /// main boss has none -- the extra is reassigned to the un-credited boss.
    fn backfill_tomb_of_the_ancients(&mut self) -> SqlResult<()> {
        struct Bag {
            id: i64,
            mob_type: i32,
            is_brown: bool,
            items: Vec<i32>,
        }

        // Group every Tomb bag by run, preserving insertion order within a run.
        let mut runs: std::collections::HashMap<i32, Vec<Bag>> = std::collections::HashMap::new();
        let mut run_order: Vec<i32> = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT d.id, d.map_seed, d.mob_type, d.bag_type, i.item_id
                 FROM loot_drops d
                 LEFT JOIN loot_items i ON i.drop_id = d.id
                 WHERE d.dungeon = ?1
                 ORDER BY d.map_seed, d.id",
            )?;
            let rows = stmt.query_map(params![super::TOMB_OF_THE_ANCIENTS_NAME], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i32>(1)?,
                    r.get::<_, i32>(2)?,
                    r.get::<_, i32>(3)?,
                    r.get::<_, Option<i32>>(4)?,
                ))
            })?;
            for row in rows {
                let (id, seed, mob, bag_type, item) = row?;
                let bags = runs.entry(seed).or_insert_with(|| {
                    run_order.push(seed);
                    Vec::new()
                });
                match bags.last_mut() {
                    Some(b) if b.id == id => {
                        if let Some(it) = item {
                            b.items.push(it);
                        }
                    }
                    _ => {
                        let mut items = Vec::new();
                        if let Some(it) = item {
                            items.push(it);
                        }
                        bags.push(Bag {
                            id,
                            mob_type: mob,
                            is_brown: matches!(bag_type, 1280 | 1709),
                            items,
                        });
                    }
                }
            }
        }

        let assets = crate::assets::get_asset_manager();
        let boss_name = |object_type: i32, fallback: &str| -> String {
            assets
                .object_name(object_type)
                .unwrap_or_else(|| fallback.to_string())
        };

        let is_sarcophagus = |t: i32| {
            matches!(
                t,
                super::TOMB_ACTIVE_SARCOPHAGUS_OBJECT_TYPE
                    | super::TOMB_TREASURE_SARCOPHAGUS_OBJECT_TYPE
            )
        };

        // A bag left for the distribution pass, with the boss its proximity guess
        // preferred (if a main boss) and whether it was a Life bag stripped off a
        // non-Life-dropper (so an unrecoverable one falls back to Unknown).
        struct Floating<'a> {
            bag: &'a Bag,
            preferred: Option<i32>,
            life_misattributed: bool,
        }

        let mut updates: Vec<(i64, i32, String)> = Vec::new();
        for seed in &run_order {
            let bags = &runs[seed];
            // A seed of 0 means the run instance is unknown; distinct real runs
            // could be merged under it, so never guess by alphabetical order
            // there. Deterministic per-bag rules still apply.
            let allow_alphabetical = *seed != 0;
            // Main bosses that already hold a real bag this run. Each main boss
            // holds at most one; the Mark of Geb bag is a separate spawner bag
            // and never claims a slot.
            let mut claimed: std::collections::HashSet<i32> = std::collections::HashSet::new();
            let mut floating: Vec<Floating> = Vec::new();

            // Pass A: apply the deterministic per-bag rules and record which main
            // bosses genuinely hold a bag, before any ambiguous distribution.
            for b in bags {
                if b.is_brown {
                    continue;
                }
                // (5) Mark of Geb drops only from Geb (via an invisible spawner).
                // It is a separate spawner bag, not one of Geb's own loot bags,
                // so it does not consume Geb's slot.
                if b.items.contains(&super::MARK_OF_GEB_ITEM_ID) {
                    if b.mob_type != super::TOMB_GEB_OBJECT_TYPE {
                        updates.push((
                            b.id,
                            super::TOMB_GEB_OBJECT_TYPE,
                            boss_name(super::TOMB_GEB_OBJECT_TYPE, "Geb"),
                        ));
                    }
                    continue;
                }

                // A bag holding only "modifier spawner" loot (pet food, eggs, skin
                // tokens, effusions, mystery-stat crates) comes from an invisible
                // spawner, not a boss. Credit it to Geb (like the Mark) without
                // claiming a slot, so it does not steal a real boss bag's slot.
                if super::is_tomb_spawner_only_bag(&b.items) {
                    if b.mob_type != super::TOMB_GEB_OBJECT_TYPE {
                        updates.push((
                            b.id,
                            super::TOMB_GEB_OBJECT_TYPE,
                            boss_name(super::TOMB_GEB_OBJECT_TYPE, "Geb"),
                        ));
                    }
                    continue;
                }

                // The Sarcophagus miniboss (Active/Treasure) is a legitimate
                // separate Life dropper; its bags are left untouched and never
                // count against the three main bosses' one-bag cap.
                if is_sarcophagus(b.mob_type) {
                    continue;
                }

                // (3) A unique drop signature names the boss outright and claims
                // its slot, overriding any proximity guess.
                if let Some((t, n)) = super::tomb_unique_boss(&b.items) {
                    if super::is_tomb_main_boss(t) {
                        claimed.insert(t);
                    }
                    if b.mob_type != t {
                        updates.push((b.id, t, n));
                    }
                    continue;
                }

                // Everything else is ambiguous: a Life/junk bag whose boss can
                // only be guessed. Defer to the distribution pass.
                let has_life = b.items.contains(&super::POTION_OF_LIFE_ITEM_ID);
                if super::is_tomb_main_boss(b.mob_type) {
                    floating.push(Floating {
                        bag: b,
                        preferred: Some(b.mob_type),
                        life_misattributed: false,
                    });
                } else if b.mob_type == 0 {
                    floating.push(Floating {
                        bag: b,
                        preferred: None,
                        life_misattributed: false,
                    });
                } else if has_life {
                    // (1) Life never drops from artifacts/minions: strip the wrong
                    // credit and re-resolve.
                    floating.push(Floating {
                        bag: b,
                        preferred: None,
                        life_misattributed: true,
                    });
                }
                // A non-Life bag credited to a non-boss (e.g. an artifact) is left
                // as-is: nothing marks it as a boss drop.
            }

            // Pass B: distribute the ambiguous bags one per main boss. Reserve
            // slots for bags whose proximity already named a main boss before
            // distributing orphan (Unknown) bags, so a correctly-stored main-boss
            // bag is never bumped by an orphan that happens to have a lower id.
            // Keep a bag's proximity boss when that boss is still un-claimed;
            // otherwise hand it to the alphabetically-first un-claimed main boss
            // so a boss never keeps two bags while another main boss has none.
            floating.sort_by_key(|f| f.preferred.is_none());
            for f in floating {
                if let Some(p) = f.preferred {
                    if !claimed.contains(&p) {
                        claimed.insert(p);
                        // Already stored as this boss; no update needed.
                        continue;
                    }
                }

                let placed = if allow_alphabetical {
                    super::TOMB_MAIN_BOSSES
                        .iter()
                        .find(|(t, _)| !claimed.contains(t))
                        .copied()
                } else {
                    None
                };

                if let Some((t, n)) = placed {
                    claimed.insert(t);
                    if f.bag.mob_type != t {
                        updates.push((f.bag.id, t, boss_name(t, n)));
                    }
                    continue;
                }

                // No free main boss to take it. A stripped-artifact Life bag must
                // not keep its wrong credit; a bag already on a (now full) main
                // boss or an Unknown bag simply keeps its current attribution.
                if f.life_misattributed && f.bag.mob_type != 0 {
                    updates.push((f.bag.id, 0, "Unknown".to_string()));
                }
            }
        }

        for (id, t, n) in updates {
            self.conn.execute(
                "UPDATE loot_drops SET mob_type = ?1, mob_name = ?2 WHERE id = ?3",
                params![t, n, id],
            )?;
        }
        Ok(())
    }

    /// Backfill Ice Tomb boss attribution for legacy bags, mirroring the live
    /// [`resolve_ice_tomb_attribution`](super::tracker) rules on stored history.
    /// Bags are grouped by run (`map_seed`); a unique per-boss ring names its
    /// boss, then remaining Unknown bags that hold boss loot are distributed one
    /// per boss to the still-un-credited bosses alphabetically. Bags already on
    /// an Ice Tomb boss seed the run and are left unchanged (re-run-safe). The
    /// event chest and other correctly-attributed non-boss bags are untouched.
    fn backfill_ice_tomb(&mut self) -> SqlResult<()> {
        struct Bag {
            id: i64,
            mob_type: i32,
            is_brown: bool,
            items: Vec<i32>,
        }

        // Group every Ice Tomb bag by run, preserving insertion order.
        let mut runs: std::collections::HashMap<i32, Vec<Bag>> = std::collections::HashMap::new();
        let mut run_order: Vec<i32> = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT d.id, d.map_seed, d.mob_type, d.bag_type, i.item_id
                 FROM loot_drops d
                 LEFT JOIN loot_items i ON i.drop_id = d.id
                 WHERE d.dungeon = ?1
                 ORDER BY d.map_seed, d.id",
            )?;
            let rows = stmt.query_map(params![super::ICE_TOMB_NAME], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i32>(1)?,
                    r.get::<_, i32>(2)?,
                    r.get::<_, i32>(3)?,
                    r.get::<_, Option<i32>>(4)?,
                ))
            })?;
            for row in rows {
                let (id, seed, mob, bag_type, item) = row?;
                let bags = runs.entry(seed).or_insert_with(|| {
                    run_order.push(seed);
                    Vec::new()
                });
                match bags.last_mut() {
                    Some(b) if b.id == id => {
                        if let Some(it) = item {
                            b.items.push(it);
                        }
                    }
                    _ => {
                        let mut items = Vec::new();
                        if let Some(it) = item {
                            items.push(it);
                        }
                        bags.push(Bag {
                            id,
                            mob_type: mob,
                            is_brown: matches!(bag_type, 1280 | 1709),
                            items,
                        });
                    }
                }
            }
        }

        // Resolve a boss's stored (object_type, name). The real boss object id
        // is stored so the loot history draws the boss sprite.
        let boss_output = |boss: &'static str| -> (i32, String) {
            (super::ice_tomb_object_type_for_boss(boss), boss.to_string())
        };
        // Whether a stored mob_type already names an Ice Tomb boss -- either its
        // boss id (a fixed run) or its soul id (a run fixed by an earlier build)
        // -- so it seeds the run instead of being re-resolved.
        let stored_boss = |mob_type: i32| -> Option<&'static str> {
            super::ice_tomb_boss_name_for_object(mob_type)
                .or_else(|| super::ice_tomb_boss_name_for_soul(mob_type))
        };

        let mut updates: Vec<(i64, i32, String)> = Vec::new();
        for seed in &run_order {
            let bags = &runs[seed];
            let allow_distribute = *seed != 0;
            let mut credited: std::collections::HashSet<&'static str> =
                std::collections::HashSet::new();
            let mut floating: Vec<&Bag> = Vec::new();

            // Pass A: seed credited bosses and apply the unique-ring signature.
            for b in bags {
                if b.is_brown {
                    continue;
                }
                // The unique ring is ground truth and overrides any (possibly
                // mis-tagged) stored soul.
                if let Some(boss) = super::ice_tomb_unique_boss(&b.items) {
                    credited.insert(boss);
                    let (t, n) = boss_output(boss);
                    if b.mob_type != t {
                        updates.push((b.id, t, n));
                    }
                    continue;
                }
                if let Some(boss) = stored_boss(b.mob_type) {
                    credited.insert(boss);
                    continue;
                }
                // Only Unknown bags that hold Ice Tomb boss loot are ambiguous
                // soul bags to distribute; anything else keeps its attribution.
                if b.mob_type == 0 && super::ice_tomb_has_boss_loot(&b.items) {
                    floating.push(b);
                }
            }

            if !allow_distribute {
                continue;
            }

            // Pass B: hand each ambiguous bag to a boss without one yet.
            for b in floating {
                let Some(boss) = super::ICE_TOMB_BOSSES
                    .iter()
                    .copied()
                    .find(|b| !credited.contains(b))
                else {
                    break;
                };
                credited.insert(boss);
                let (t, n) = boss_output(boss);
                if b.mob_type != t {
                    updates.push((b.id, t, n));
                }
            }
        }

        for (id, t, n) in updates {
            self.conn.execute(
                "UPDATE loot_drops SET mob_type = ?1, mob_name = ?2 WHERE id = ?3",
                params![t, n, id],
            )?;
        }
        Ok(())
    }

    /// Backfill boss attribution for Unknown, non-brown bags whose items are a
    /// unique boss drop in their dungeon. Ambiguous bags stay Unknown.
    fn backfill_unique_boss_drops(&mut self) -> SqlResult<()> {
        struct Bag {
            id: i64,
            dungeon: String,
            is_brown: bool,
            items: Vec<i32>,
        }

        let mut bags: Vec<Bag> = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT d.id, d.dungeon, d.bag_type, i.item_id
                 FROM loot_drops d
                 LEFT JOIN loot_items i ON i.drop_id = d.id
                 WHERE d.mob_type = 0
                 ORDER BY d.id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i32>(2)?,
                    r.get::<_, Option<i32>>(3)?,
                ))
            })?;
            for row in rows {
                let (id, dungeon, bag_type, item) = row?;
                match bags.last_mut() {
                    Some(b) if b.id == id => {
                        if let Some(it) = item {
                            b.items.push(it);
                        }
                    }
                    _ => {
                        let mut items = Vec::new();
                        if let Some(it) = item {
                            items.push(it);
                        }
                        bags.push(Bag {
                            id,
                            dungeon,
                            is_brown: matches!(bag_type, 1280 | 1709),
                            items,
                        });
                    }
                }
            }
        }

        let mut updates: Vec<(i64, i32, String)> = Vec::new();
        for b in &bags {
            if b.is_brown || b.items.is_empty() || super::has_curated_boss_attribution(&b.dungeon) {
                continue;
            }
            if let Some((t, n)) = super::unique_boss_drop(&b.dungeon, &b.items) {
                updates.push((b.id, t, n));
            }
        }
        for (id, t, n) in updates {
            self.conn.execute(
                "UPDATE loot_drops SET mob_type = ?1, mob_name = ?2 WHERE id = ?3",
                params![t, n, id],
            )?;
        }
        Ok(())
    }

    /// Backfill Unknown loot sources (`?`, `mob_type = 0`) by correlating each
    /// still-unattributed non-brown bag with a boss the Combat History recorded
    /// as killed in the same map instance. Reuses the live attribution rules
    /// ([`LootTracker::resolve_boss_override_with`]) with the recorded fights as
    /// the kill source, so history gains exactly the attributions the live
    /// tracker would now produce (bags whose boss died offscreen, and ambiguous
    /// drop-table bags a confirmed kill disambiguates). Runs once; if no Combat
    /// History exists yet there is nothing to correlate and bags are left as-is.
    fn backfill_unknown_from_combat_fights(&mut self) -> SqlResult<bool> {
        let (kills, readable) = self.load_recorded_boss_kills();
        if !readable {
            return Ok(false);
        }
        self.backfill_unknown_with_kills(&kills)?;
        Ok(true)
    }

    /// Correlation core of [`backfill_unknown_from_combat_fights`], taking the
    /// recorded kills explicitly so it can be unit-tested against an in-memory
    /// database without touching the on-disk combat history.
    fn backfill_unknown_with_kills(&mut self, kills: &[super::RecentBossKill]) -> SqlResult<()> {
        if kills.is_empty() {
            return Ok(());
        }

        struct Bag {
            id: i64,
            dungeon: String,
            map_seed: i32,
            timestamp: i64,
            bag_type: i32,
            items: Vec<i32>,
        }

        let mut bags: Vec<Bag> = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT d.id, d.dungeon, d.map_seed, d.timestamp, d.bag_type, i.item_id
                 FROM loot_drops d
                 LEFT JOIN loot_items i ON i.drop_id = d.id
                 WHERE d.mob_type = 0
                 ORDER BY d.id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i32>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i32>(4)?,
                    r.get::<_, Option<i32>>(5)?,
                ))
            })?;
            for row in rows {
                let (id, dungeon, map_seed, timestamp, bag_type, item) = row?;
                match bags.last_mut() {
                    Some(b) if b.id == id => {
                        if let Some(it) = item {
                            b.items.push(it);
                        }
                    }
                    _ => {
                        let mut items = Vec::new();
                        if let Some(it) = item {
                            items.push(it);
                        }
                        bags.push(Bag {
                            id,
                            dungeon,
                            map_seed,
                            timestamp,
                            bag_type,
                            items,
                        });
                    }
                }
            }
        }

        let mut updates: Vec<(i64, i32, String)> = Vec::new();
        for b in &bags {
            // Dungeons with their own curated backfill (LoD, SpecPen, Tomb,
            // Moonlight Village) make deliberate per-run decisions -- including
            // leaving some bags Unknown -- and their fight-correlation is scoped
            // to a live in-memory window. Replaying the live rules here over the
            // full recorded-fight set risks overriding those decisions or, for
            // seed-only matches like Moonlight's, borrowing an unrelated fight.
            // Leave them to their dedicated migrations / the live tracker. This
            // pass handles the general offscreen-death case: a bag whose items a
            // boss is known to drop in this dungeon, disambiguated by a confirmed
            // kill in the same instance.
            if crate::loot::has_curated_boss_attribution(&b.dungeon) {
                continue;
            }
            let Some(bag_type) = LootBagType::from_id(b.bag_type) else {
                continue;
            };
            if let Some((t, n)) = LootTracker::resolve_boss_override_with(
                kills,
                bag_type,
                &b.dungeon,
                0,
                &b.items,
                b.map_seed,
                b.timestamp,
            ) {
                updates.push((b.id, t, n));
            }
        }
        for (id, t, n) in updates {
            self.conn.execute(
                "UPDATE loot_drops SET mob_type = ?1, mob_name = ?2 WHERE id = ?3",
                params![t, n, id],
            )?;
        }
        Ok(())
    }

    /// Load every boss the paired Combat History recorded as killed, as the
    /// fight-correlation input for [`backfill_unknown_from_combat_fights`].
    /// Returns `(kills, readable)`. `readable` is false only when the combat
    /// database file exists but could not be read (transient lock / incompatible
    /// schema), signalling the caller to defer the backfill to a later launch. A
    /// missing file (fresh install / no fights yet) or no paired path is a
    /// legitimate no-op: `(empty, true)`.
    ///
    /// The combat path is injected (see [`Self::open_writer`]) so this reads only
    /// the caller-supplied history, never a machine-wide default; tests can point
    /// it at a hermetic temporary database.
    fn load_recorded_boss_kills(&self) -> (Vec<super::RecentBossKill>, bool) {
        let Some(path) = self.combat_path.as_deref() else {
            return (Vec::new(), true);
        };
        if !path.exists() {
            return (Vec::new(), true);
        }
        let conn = match Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    "[LOOT] Deferring Unknown-source backfill; combat DB unreadable: {e}"
                );
                return (Vec::new(), false);
            }
        };
        let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
        let mut stmt = match conn.prepare(
            "SELECT map_seed, boss_object_type, boss_name, started_at, ended_at
             FROM fights WHERE killed = 1 AND boss_object_type > 0",
        ) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    "[LOOT] Deferring Unknown-source backfill; combat DB query failed: {e}"
                );
                return (Vec::new(), false);
            }
        };
        let rows = match stmt.query_map([], |r| {
            Ok(super::RecentBossKill {
                map_seed: r.get(0)?,
                object_type: r.get(1)?,
                name: r.get(2)?,
                started_at_ms: r.get(3)?,
                ended_at_ms: r.get(4)?,
            })
        }) {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(
                    "[LOOT] Deferring Unknown-source backfill; combat DB read failed: {e}"
                );
                return (Vec::new(), false);
            }
        };
        let mut kills = Vec::new();
        for row in rows.flatten() {
            kills.push(row);
        }
        (kills, true)
    }
    /// bags. Reliable signals (Mark of the Soulwarden, per-miniboss whites and
    /// blueprints) are applied directly. Weaker signals - a stat potion, a set
    /// shard narrowing to a branch pair, or generic boss-only loot - are resolved
    /// per run against the dungeon's rules: a run holds at most two minibosses,
    /// each dropping once. A bag proven to be a miniboss drop but not otherwise
    /// identifiable defaults to Oculon (the miniboss most prone to going
    /// unrecorded) unless a shard rules him out, in which case it stays Unknown.
    fn backfill_spectral_penitentiary(&mut self) -> SqlResult<()> {
        const MINIBOSS_CLUSTER_MS: i64 = 300_000;
        const OCULON: i32 = super::SPEN_OCULON_OBJECT_TYPE;

        struct Bag {
            id: i64,
            timestamp: i64,
            mob_type: i32,
            is_brown: bool,
            items: Vec<i32>,
        }
        struct Candidate {
            id: i64,
            timestamp: i64,
            /// Minibosses this bag could belong to (a singleton when a potion or
            /// white pins it down; a branch pair or all four otherwise).
            options: Vec<(i32, &'static str)>,
        }

        let all_four: [(i32, &'static str); 4] = [
            (OCULON, "Overseer Oculon"),
            (super::SPEN_GRETCH_OBJECT_TYPE, "Groundskeeper Gretch"),
            (super::SPEN_ZOLE_OBJECT_TYPE, "Griefkeeper Zole"),
            (super::SPEN_LOBOTOMIK_OBJECT_TYPE, "Doctor Lobotomik"),
        ];

        // Load every SpecPen bag grouped by run (map_seed).
        let mut runs: std::collections::HashMap<i32, Vec<Bag>> = std::collections::HashMap::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT d.id, d.map_seed, d.timestamp, d.mob_type, d.bag_type,
                        i.item_id
                 FROM loot_drops d
                 LEFT JOIN loot_items i ON i.drop_id = d.id
                 WHERE d.dungeon = ?1
                 ORDER BY d.map_seed, d.id",
            )?;
            let rows = stmt.query_map(params![super::SPECTRAL_PENITENTIARY_NAME], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i32>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i32>(3)?,
                    r.get::<_, i32>(4)?,
                    r.get::<_, Option<i32>>(5)?,
                ))
            })?;
            for row in rows {
                let (id, seed, ts, mob, bag_type, item) = row?;
                let bags = runs.entry(seed).or_default();
                match bags.last_mut() {
                    Some(b) if b.id == id => {
                        if let Some(it) = item {
                            b.items.push(it);
                        }
                    }
                    _ => {
                        let mut items = Vec::new();
                        if let Some(it) = item {
                            items.push(it);
                        }
                        bags.push(Bag {
                            id,
                            timestamp: ts,
                            mob_type: mob,
                            is_brown: matches!(bag_type, 1280 | 1709),
                            items,
                        });
                    }
                }
            }
        }

        let mut updates: Vec<(i64, i32, &'static str)> = Vec::new();
        for (_seed, bags) in runs {
            // First pass: the Mark of the Soulwarden is unique to Murcian, so any
            // bag holding it is a Murcian drop with certainty. Resolve these
            // before every other rule and exclude them from later passes.
            let mut murcian_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
            for b in &bags {
                if b.mob_type != 0 || b.is_brown {
                    continue;
                }
                if let Some((t, n)) = super::spectral_penitentiary_murcian(&b.items) {
                    updates.push((b.id, t, n));
                    murcian_ids.insert(b.id);
                }
            }

            // Minibosses the run definitely contains: proximity-attributed plus
            // any pinned down by a reliable white/blueprint.
            let mut confirmed: std::collections::HashSet<i32> = std::collections::HashSet::new();
            for b in &bags {
                if super::is_spectral_penitentiary_miniboss(b.mob_type) {
                    confirmed.insert(b.mob_type);
                }
            }
            let mut candidates: Vec<Candidate> = Vec::new();
            for b in &bags {
                if b.mob_type != 0 || b.is_brown || murcian_ids.contains(&b.id) {
                    continue;
                }
                if let Some((t, n)) = super::spectral_penitentiary_reliable_boss(&b.items) {
                    updates.push((b.id, t, n));
                    if super::is_spectral_penitentiary_miniboss(t) {
                        confirmed.insert(t);
                    }
                    continue;
                }
                let boss_loot = super::spectral_penitentiary_boss_confirmed(&b.items);
                if let Some((t, n)) =
                    super::spectral_penitentiary_potion_miniboss(&b.items, boss_loot)
                {
                    candidates.push(Candidate {
                        id: b.id,
                        timestamp: b.timestamp,
                        options: vec![(t, n)],
                    });
                } else if boss_loot {
                    // Proven miniboss drop, but no potion identity: narrow by set
                    // shard if present, otherwise leave it open to all four.
                    let options = super::spectral_penitentiary_shard_pair(&b.items)
                        .map(|p| p.to_vec())
                        .unwrap_or_else(|| all_four.to_vec());
                    candidates.push(Candidate {
                        id: b.id,
                        timestamp: b.timestamp,
                        options,
                    });
                }
                // Otherwise (lone stat potion, or no boss-only loot) it may be a
                // minion drop; leave it Unknown.
            }

            // Rule A: at most two minibosses per run. Confirmed ones hold slots
            // first; strong (single-option) potion candidates claim remaining
            // slots by frequency; finally an Oculon default fills a slot only for
            // a proven-miniboss bag not already explained by a confirmed boss.
            let mut allowed = confirmed.clone();
            if allowed.len() < 2 {
                let mut freq: std::collections::HashMap<i32, usize> =
                    std::collections::HashMap::new();
                for c in &candidates {
                    if c.options.len() == 1 {
                        *freq.entry(c.options[0].0).or_default() += 1;
                    }
                }
                let mut ranked: Vec<(i32, usize)> = freq.into_iter().collect();
                ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                for (m, _) in ranked {
                    if allowed.len() >= 2 {
                        break;
                    }
                    allowed.insert(m);
                }
            }
            let needs_oculon_default = candidates.iter().any(|c| {
                c.options.iter().any(|(t, _)| *t == OCULON)
                    && !c.options.iter().any(|(t, _)| confirmed.contains(t))
            });
            if allowed.len() < 2 && needs_oculon_default {
                allowed.insert(OCULON);
            }

            // Assign each candidate to a feasible miniboss, preferring one the
            // run already confirmed, then the Oculon default, then a sole option.
            let mut assigned: std::collections::HashMap<i32, Vec<(i64, i64, &'static str)>> =
                std::collections::HashMap::new();
            for c in &candidates {
                let feasible: Vec<(i32, &'static str)> = c
                    .options
                    .iter()
                    .copied()
                    .filter(|(t, _)| allowed.contains(t))
                    .collect();
                let pick = feasible
                    .iter()
                    .find(|(t, _)| confirmed.contains(t))
                    .or_else(|| feasible.iter().find(|(t, _)| *t == OCULON))
                    .or(if feasible.len() == 1 {
                        feasible.first()
                    } else {
                        None
                    })
                    .copied();
                if let Some((t, n)) = pick {
                    assigned.entry(t).or_default().push((c.timestamp, c.id, n));
                }
            }

            // Rule B: a miniboss drops once, so its bags cluster in time. Reject
            // assignments far from that miniboss's cluster median.
            for (t, mut group) in assigned {
                group.sort_by_key(|(ts, _, _)| *ts);
                let median = group[group.len() / 2].0;
                for (ts, id, name) in group {
                    if (ts - median).abs() <= MINIBOSS_CLUSTER_MS {
                        updates.push((id, t, name));
                    }
                }
            }
        }

        for (id, t, n) in updates {
            self.conn.execute(
                "UPDATE loot_drops SET mob_type = ?1, mob_name = ?2 WHERE id = ?3",
                params![t, n, id],
            )?;
        }
        Ok(())
    }

    /// Reclassify Unknown (`mob_type = 0`) bags in `dungeon` to the boss its
    /// items identify via `resolve`. When `exclude_brown` is set, brown bags are
    /// skipped (bosses never drop loot in brown bags, so those are public bags).
    fn backfill_unknown_by_item_signal(
        &mut self,
        dungeon: &str,
        exclude_brown: bool,
        resolve: impl Fn(&[i32]) -> Option<(i32, &'static str)>,
    ) -> SqlResult<()> {
        let mut updates: Vec<(i64, i32, &'static str)> = Vec::new();
        {
            let sql = if exclude_brown {
                "SELECT d.id, i.item_id FROM loot_drops d
                 JOIN loot_items i ON i.drop_id = d.id
                 WHERE d.mob_type = 0 AND d.dungeon = ?1 AND d.bag_type NOT IN (1280, 1709)
                 ORDER BY d.id"
            } else {
                "SELECT d.id, i.item_id FROM loot_drops d
                 JOIN loot_items i ON i.drop_id = d.id
                 WHERE d.mob_type = 0 AND d.dungeon = ?1
                 ORDER BY d.id"
            };
            let mut stmt = self.conn.prepare(sql)?;
            let rows = stmt.query_map(params![dungeon], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i32>(1)?))
            })?;
            let mut cur_id: Option<i64> = None;
            let mut cur_items: Vec<i32> = Vec::new();
            for row in rows {
                let (id, item_id) = row?;
                if cur_id == Some(id) {
                    cur_items.push(item_id);
                } else {
                    if let Some(c) = cur_id {
                        if let Some((t, n)) = resolve(&cur_items) {
                            updates.push((c, t, n));
                        }
                    }
                    cur_id = Some(id);
                    cur_items = vec![item_id];
                }
            }
            if let Some(c) = cur_id {
                if let Some((t, n)) = resolve(&cur_items) {
                    updates.push((c, t, n));
                }
            }
        }
        for (id, t, n) in updates {
            self.conn.execute(
                "UPDATE loot_drops SET mob_type = ?1, mob_name = ?2 WHERE id = ?3",
                params![t, n, id],
            )?;
        }
        Ok(())
    }

    /// Insert a new loot drop with all its items.
    /// Returns the new drop's ID.
    pub fn insert_loot_drop(&mut self, drop: &NewLootDrop) -> SqlResult<i64> {
        let tx = self.conn.transaction()?;

        let drop_id = Self::insert_drop_in_tx(&tx, drop)?;

        for item in &drop.items {
            Self::insert_item_in_tx(&tx, drop_id, item)?;
        }

        tx.commit()?;
        Ok(drop_id)
    }

    /// Insert drop record in transaction.
    fn insert_drop_in_tx(tx: &Transaction, drop: &NewLootDrop) -> SqlResult<i64> {
        tx.execute(
            r#"INSERT INTO loot_drops 
               (timestamp, dungeon, map_seed, char_id, char_name, class_id, skin_id, tex1, tex2, fame,
                is_seasonal, exalt_bonus, loot_drop_active, loot_tier_active,
                bag_type, mob_type, mob_name)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)"#,
            params![
                drop.timestamp,
                drop.player.dungeon,
                drop.player.map_seed,
                drop.player.char_id,
                drop.player.name,
                drop.player.class_id,
                drop.player.skin_id,
                drop.player.tex1 as i32,
                drop.player.tex2 as i32,
                drop.player.fame,
                drop.player.is_seasonal as i32,
                drop.player.exalt_bonus,
                drop.player.loot_drop_active as i32,
                drop.player.loot_tier_active as i32,
                drop.bag_type as i32,
                drop.mob_type,
                drop.mob_name,
            ],
        )?;
        Ok(tx.last_insert_rowid())
    }

    /// Insert item record in transaction.
    fn insert_item_in_tx(tx: &Transaction, drop_id: i64, item: &NewLootItem) -> SqlResult<()> {
        let enchant_ids_str = item
            .enchant_ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",");

        tx.execute(
            r#"INSERT INTO loot_items 
               (drop_id, slot, item_id, item_name, enchant_count, enchant_ids)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
            params![
                drop_id,
                item.slot,
                item.item_id,
                item.item_name,
                item.enchant_ids.len() as i32,
                enchant_ids_str,
            ],
        )?;
        Ok(())
    }

    /// Get recent loot drops (most recent first).
    pub fn get_recent_drops(&self, limit: i64) -> SqlResult<Vec<LootDropRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM loot_drops ORDER BY timestamp DESC LIMIT ?1")?;

        let drops = stmt
            .query_map([limit], Self::row_to_drop)?
            .collect::<SqlResult<Vec<_>>>()?;

        self.load_items_for_drops(drops)
    }

    /// Get drops by character ID.
    pub fn get_drops_by_character(
        &self,
        char_id: i32,
        limit: i64,
    ) -> SqlResult<Vec<LootDropRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM loot_drops WHERE char_id = ?1 ORDER BY timestamp DESC LIMIT ?2",
        )?;

        let drops = stmt
            .query_map([char_id as i64, limit], Self::row_to_drop)?
            .collect::<SqlResult<Vec<_>>>()?;

        self.load_items_for_drops(drops)
    }

    /// Get drops by dungeon name.
    pub fn get_drops_by_dungeon(
        &self,
        dungeon: &str,
        limit: i64,
    ) -> SqlResult<Vec<LootDropRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM loot_drops WHERE dungeon = ?1 ORDER BY timestamp DESC LIMIT ?2",
        )?;

        let drops = stmt
            .query_map(params![dungeon, limit], Self::row_to_drop)?
            .collect::<SqlResult<Vec<_>>>()?;

        self.load_items_for_drops(drops)
    }

    /// Get drops by bag type.
    pub fn get_drops_by_bag_type(
        &self,
        bag_type: LootBagType,
        limit: i64,
    ) -> SqlResult<Vec<LootDropRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM loot_drops WHERE bag_type = ?1 ORDER BY timestamp DESC LIMIT ?2",
        )?;

        let drops = stmt
            .query_map([bag_type as i64, limit], Self::row_to_drop)?
            .collect::<SqlResult<Vec<_>>>()?;

        self.load_items_for_drops(drops)
    }

    /// Get drops in date range.
    pub fn get_drops_in_range(
        &self,
        start: i64,
        end: i64,
        limit: i64,
    ) -> SqlResult<Vec<LootDropRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM loot_drops WHERE timestamp >= ?1 AND timestamp <= ?2 
             ORDER BY timestamp DESC LIMIT ?3",
        )?;

        let drops = stmt
            .query_map([start, end, limit], Self::row_to_drop)?
            .collect::<SqlResult<Vec<_>>>()?;

        self.load_items_for_drops(drops)
    }

    /// Get drops in date range with pagination (offset + limit).
    pub fn get_drops_in_range_paged(
        &self,
        start: i64,
        end: i64,
        offset: i64,
        limit: i64,
    ) -> SqlResult<Vec<LootDropRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM loot_drops WHERE timestamp >= ?1 AND timestamp <= ?2 
             ORDER BY timestamp DESC LIMIT ?3 OFFSET ?4",
        )?;

        let drops = stmt
            .query_map([start, end, limit, offset], Self::row_to_drop)?
            .collect::<SqlResult<Vec<_>>>()?;

        self.load_items_for_drops(drops)
    }

    /// Get drops with optional source filters (mob, dungeon, character, item, bag types).
    /// Builds dynamic SQL WHERE clause based on active filters.
    pub fn get_drops_filtered(
        &self,
        start: i64,
        end: i64,
        filters: &SourceFilters,
        offset: i64,
        limit: i64,
    ) -> SqlResult<Vec<LootDropRecord>> {
        // Build dynamic SQL with optional filter clauses
        // When filtering by item, we need to JOIN with loot_items
        let has_item_filter = filters.item_id.is_some();

        let mut sql = if has_item_filter {
            String::from(
                "SELECT DISTINCT d.* FROM loot_drops d \
                 INNER JOIN loot_items i ON i.drop_id = d.id \
                 WHERE d.timestamp >= ?1 AND d.timestamp <= ?2",
            )
        } else {
            String::from("SELECT * FROM loot_drops WHERE timestamp >= ?1 AND timestamp <= ?2")
        };

        // Table alias prefix for column references when using JOIN
        let col_prefix = if has_item_filter { "d." } else { "" };

        // Track parameter index (starts at 3 since ?1 and ?2 are start/end)
        let mut param_idx = 3;

        if filters.mob_type.is_some() {
            sql.push_str(&format!(" AND {}mob_type = ?{}", col_prefix, param_idx));
            param_idx += 1;
        }
        if filters.dungeon.is_some() {
            sql.push_str(&format!(" AND {}dungeon = ?{}", col_prefix, param_idx));
            param_idx += 1;
        }
        if filters.char_id.is_some() {
            sql.push_str(&format!(" AND {}char_id = ?{}", col_prefix, param_idx));
            param_idx += 1;
        }
        if filters.item_id.is_some() {
            sql.push_str(&format!(" AND i.item_id = ?{}", param_idx));
            param_idx += 1;
        }

        // Free-text search across mob name, dungeon, and item name.
        let search_like = filters
            .search_text
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| format!("%{}%", s));
        if search_like.is_some() {
            // Qualify the outer drops id so it isn't shadowed by loot_items.id
            // inside the correlated EXISTS subquery.
            let drop_id_ref = if has_item_filter {
                "d.id"
            } else {
                "loot_drops.id"
            };
            sql.push_str(&format!(
                " AND ({p}mob_name LIKE ?{i} OR {p}dungeon LIKE ?{i} \
                 OR EXISTS (SELECT 1 FROM loot_items li WHERE li.drop_id = {d} AND li.item_name LIKE ?{i}))",
                p = col_prefix,
                d = drop_id_ref,
                i = param_idx
            ));
            param_idx += 1;
        }

        // Add bag type filter with IN clause.
        // Some(non-empty) => restrict to those bag types.
        // Some(empty) => user disabled all bag types, so match nothing.
        // None => no bag-type restriction.
        if let Some(ref bag_types) = filters.bag_types {
            if !bag_types.is_empty() {
                let placeholders: Vec<String> = (0..bag_types.len())
                    .map(|i| format!("?{}", param_idx + i))
                    .collect();
                sql.push_str(&format!(
                    " AND {}bag_type IN ({})",
                    col_prefix,
                    placeholders.join(", ")
                ));
                param_idx += bag_types.len();
            } else {
                sql.push_str(" AND 1=0");
            }
        }

        sql.push_str(&format!(
            " ORDER BY {}timestamp DESC LIMIT ?{} OFFSET ?{}",
            col_prefix,
            param_idx,
            param_idx + 1
        ));

        let mut stmt = self.conn.prepare(&sql)?;

        // Build parameter list dynamically using boxed trait objects
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        params.push(Box::new(start));
        params.push(Box::new(end));

        if let Some(mob) = filters.mob_type {
            params.push(Box::new(mob));
        }
        if let Some(ref dung) = filters.dungeon {
            params.push(Box::new(dung.clone()));
        }
        if let Some(char) = filters.char_id {
            params.push(Box::new(char));
        }
        if let Some(item) = filters.item_id {
            params.push(Box::new(item));
        }
        if let Some(ref like) = search_like {
            params.push(Box::new(like.clone()));
        }
        if let Some(ref bag_types) = filters.bag_types {
            for &bt in bag_types {
                params.push(Box::new(bt));
            }
        }
        params.push(Box::new(limit));
        params.push(Box::new(offset));

        // Convert to slice of references for rusqlite
        let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let drops = stmt
            .query_map(param_refs.as_slice(), Self::row_to_drop)?
            .collect::<SqlResult<Vec<_>>>()?;

        self.load_items_for_drops(drops)
    }

    /// Count drops matching the given filters (for showing total in UI).
    pub fn count_drops_filtered(
        &self,
        start: i64,
        end: i64,
        filters: &SourceFilters,
    ) -> SqlResult<i64> {
        // Build dynamic SQL with optional filter clauses
        // When filtering by item, we need to JOIN with loot_items
        let has_item_filter = filters.item_id.is_some();

        let mut sql = if has_item_filter {
            String::from(
                "SELECT COUNT(DISTINCT d.id) FROM loot_drops d \
                 INNER JOIN loot_items i ON i.drop_id = d.id \
                 WHERE d.timestamp >= ?1 AND d.timestamp <= ?2",
            )
        } else {
            String::from(
                "SELECT COUNT(*) FROM loot_drops WHERE timestamp >= ?1 AND timestamp <= ?2",
            )
        };

        // Table alias prefix for column references when using JOIN
        let col_prefix = if has_item_filter { "d." } else { "" };

        // Track parameter index (starts at 3 since ?1 and ?2 are start/end)
        let mut param_idx = 3;

        if filters.mob_type.is_some() {
            sql.push_str(&format!(" AND {}mob_type = ?{}", col_prefix, param_idx));
            param_idx += 1;
        }
        if filters.dungeon.is_some() {
            sql.push_str(&format!(" AND {}dungeon = ?{}", col_prefix, param_idx));
            param_idx += 1;
        }
        if filters.char_id.is_some() {
            sql.push_str(&format!(" AND {}char_id = ?{}", col_prefix, param_idx));
            param_idx += 1;
        }
        if filters.item_id.is_some() {
            sql.push_str(&format!(" AND i.item_id = ?{}", param_idx));
            param_idx += 1;
        }

        // Free-text search across mob name, dungeon, and item name.
        let search_like = filters
            .search_text
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| format!("%{}%", s));
        if search_like.is_some() {
            let drop_id_ref = if has_item_filter {
                "d.id"
            } else {
                "loot_drops.id"
            };
            sql.push_str(&format!(
                " AND ({p}mob_name LIKE ?{i} OR {p}dungeon LIKE ?{i} \
                 OR EXISTS (SELECT 1 FROM loot_items li WHERE li.drop_id = {d} AND li.item_name LIKE ?{i}))",
                p = col_prefix,
                d = drop_id_ref,
                i = param_idx
            ));
            param_idx += 1;
        }

        // Add bag type filter with IN clause.
        // Some(non-empty) => restrict to those bag types.
        // Some(empty) => user disabled all bag types, so match nothing.
        // None => no bag-type restriction.
        if let Some(ref bag_types) = filters.bag_types {
            if !bag_types.is_empty() {
                let placeholders: Vec<String> = (0..bag_types.len())
                    .map(|i| format!("?{}", param_idx + i))
                    .collect();
                sql.push_str(&format!(
                    " AND {}bag_type IN ({})",
                    col_prefix,
                    placeholders.join(", ")
                ));
            } else {
                sql.push_str(" AND 1=0");
            }
        }

        let mut stmt = self.conn.prepare(&sql)?;

        // Build parameter list dynamically using boxed trait objects
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        params.push(Box::new(start));
        params.push(Box::new(end));

        if let Some(mob) = filters.mob_type {
            params.push(Box::new(mob));
        }
        if let Some(ref dung) = filters.dungeon {
            params.push(Box::new(dung.clone()));
        }
        if let Some(char) = filters.char_id {
            params.push(Box::new(char));
        }
        if let Some(item) = filters.item_id {
            params.push(Box::new(item));
        }
        if let Some(ref like) = search_like {
            params.push(Box::new(like.clone()));
        }
        if let Some(ref bag_types) = filters.bag_types {
            for &bt in bag_types {
                params.push(Box::new(bt));
            }
        }

        // Convert to slice of references for rusqlite
        let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        stmt.query_row(param_refs.as_slice(), |row| row.get(0))
    }

    /// Convert a database row to LootDropRecord (without items).
    fn row_to_drop(row: &rusqlite::Row) -> SqlResult<LootDropRecord> {
        // Use column names for robustness during migrations
        // skin_id might not exist in older databases, default to 0
        let skin_id: i32 = row.get::<_, i32>("skin_id").unwrap_or(0);
        let tex1: u32 = row.get::<_, i32>("tex1").unwrap_or(0) as u32;
        let tex2: u32 = row.get::<_, i32>("tex2").unwrap_or(0) as u32;

        Ok(LootDropRecord {
            id: row.get("id")?,
            timestamp: row.get("timestamp")?,
            dungeon: row.get("dungeon")?,
            map_seed: row.get("map_seed")?,
            char_id: row.get("char_id")?,
            char_name: row.get("char_name")?,
            class_id: row.get("class_id")?,
            skin_id,
            tex1,
            tex2,
            fame: row.get("fame")?,
            is_seasonal: row.get::<_, i32>("is_seasonal")? != 0,
            exalt_bonus: row.get("exalt_bonus")?,
            loot_drop_active: row.get::<_, i32>("loot_drop_active")? != 0,
            loot_tier_active: row.get::<_, i32>("loot_tier_active")? != 0,
            bag_type: row.get("bag_type")?,
            mob_type: row.get("mob_type")?,
            mob_name: row.get("mob_name")?,
            items: Vec::new(),
        })
    }

    /// Load items for a list of drops.
    fn load_items_for_drops(
        &self,
        mut drops: Vec<LootDropRecord>,
    ) -> SqlResult<Vec<LootDropRecord>> {
        for drop in &mut drops {
            drop.items = self.get_items_for_drop(drop.id)?;
        }
        Ok(drops)
    }

    /// Find every loot drop belonging to one boss fight, matched by map instance
    /// (`map_seed`), one of the fight's boss/mob types, and a drop time inside
    /// `[time_lo, time_hi]`. A `map_seed` of 0 or an empty `boss_types` never
    /// matches, since the instance is unknown. Results carry their items and are
    /// ordered oldest-first. Dungeon name is intentionally NOT part of the key:
    /// the loot and combat subsystems can store it differently (e.g. a raw
    /// `{s.wine_cellar}` token vs the resolved `Wine Cellar`), and the random
    /// per-instance seed already disambiguates the map.
    pub fn find_drops_for_run(
        &self,
        map_seed: i32,
        boss_types: &[i32],
        time_lo: i64,
        time_hi: i64,
    ) -> SqlResult<Vec<LootDropRecord>> {
        let types: Vec<i32> = boss_types.iter().copied().filter(|t| *t > 0).collect();
        if map_seed == 0 || types.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders: Vec<String> = (0..types.len()).map(|i| format!("?{}", 4 + i)).collect();
        let sql = format!(
            "SELECT * FROM loot_drops
             WHERE map_seed = ?1 AND timestamp >= ?2 AND timestamp <= ?3
               AND mob_type IN ({})
             ORDER BY timestamp ASC, id ASC",
            placeholders.join(", ")
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        params.push(Box::new(map_seed));
        params.push(Box::new(time_lo));
        params.push(Box::new(time_hi));
        for t in &types {
            params.push(Box::new(*t));
        }
        let drops = stmt
            .query_map(
                rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
                Self::row_to_drop,
            )?
            .collect::<SqlResult<Vec<_>>>()?;
        self.load_items_for_drops(drops)
    }

    /// Get items for a specific drop.
    fn get_items_for_drop(&self, drop_id: i64) -> SqlResult<Vec<LootItemRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM loot_items WHERE drop_id = ?1 ORDER BY slot")?;

        let result = stmt
            .query_map([drop_id], |row| {
                let enchant_ids_str: String = row.get(6)?;
                let parsed_enchant_ids = if enchant_ids_str.is_empty() {
                    Vec::new()
                } else {
                    enchant_ids_str
                        .split(',')
                        .filter_map(|s| s.trim().parse().ok())
                        .collect()
                };
                Ok(LootItemRecord {
                    id: row.get(0)?,
                    drop_id: row.get(1)?,
                    slot: row.get(2)?,
                    item_id: row.get(3)?,
                    item_name: row.get(4)?,
                    enchant_count: row.get(5)?,
                    enchant_ids: enchant_ids_str,
                    parsed_enchant_ids,
                })
            })?
            .collect();
        result
    }

    // ========== Aggregation Queries ==========

    /// Count total drops by bag type.
    pub fn count_drops_by_bag_type(&self) -> SqlResult<Vec<(i32, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT bag_type, COUNT(*) FROM loot_drops GROUP BY bag_type ORDER BY bag_type",
        )?;

        let result = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect();
        result
    }

    /// Count total drops by dungeon.
    pub fn count_drops_by_dungeon(&self) -> SqlResult<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT dungeon, COUNT(*) FROM loot_drops GROUP BY dungeon ORDER BY COUNT(*) DESC",
        )?;

        let result = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect();
        result
    }

    /// Count total drops.
    pub fn total_drops(&self) -> SqlResult<i64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM loot_drops", [], |row| row.get(0))
    }

    /// Count drops of specific bag types (white bags, orange bags, etc.).
    pub fn count_bag_type(&self, bag_type: LootBagType) -> SqlResult<i64> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM loot_drops WHERE bag_type = ?1",
            [bag_type as i32],
            |row| row.get(0),
        )
    }

    /// Get total white bags (regular + boosted).
    pub fn total_white_bags(&self) -> SqlResult<i64> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM loot_drops WHERE bag_type IN (?1, ?2)",
            [LootBagType::White as i32, LootBagType::BoostedWhite as i32],
            |row| row.get(0),
        )
    }

    /// Get total orange bags (regular + boosted).
    pub fn total_orange_bags(&self) -> SqlResult<i64> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM loot_drops WHERE bag_type IN (?1, ?2)",
            [
                LootBagType::Orange as i32,
                LootBagType::BoostedOrange as i32,
            ],
            |row| row.get(0),
        )
    }

    /// Get total red bags (regular + boosted).
    pub fn total_red_bags(&self) -> SqlResult<i64> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM loot_drops WHERE bag_type IN (?1, ?2)",
            [LootBagType::Red as i32, LootBagType::BoostedRed as i32],
            |row| row.get(0),
        )
    }

    /// Get statistics summary.
    pub fn get_statistics(&self) -> SqlResult<LootStatistics> {
        Ok(LootStatistics {
            total_drops: self.total_drops()?,
            white_bags: self.total_white_bags()?,
            orange_bags: self.total_orange_bags()?,
            red_bags: self.total_red_bags()?,
            by_bag_type: self.count_drops_by_bag_type()?,
            by_dungeon: self.count_drops_by_dungeon()?,
        })
    }

    // ========== Distinct Value Queries (for search autocomplete) ==========

    /// Get all distinct dungeon names from loot history.
    /// Returns sorted alphabetically for consistent autocomplete.
    pub fn get_distinct_dungeons(&self) -> SqlResult<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT dungeon FROM loot_drops ORDER BY dungeon ASC")?;
        let result = stmt.query_map([], |row| row.get(0))?.collect();
        result
    }

    /// Get all distinct mobs from loot history.
    /// Returns (mob_type, mob_name) pairs sorted by name for autocomplete.
    pub fn get_distinct_mobs(&self) -> SqlResult<Vec<(i32, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT mob_type, mob_name FROM loot_drops ORDER BY mob_name ASC")?;
        let result = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect();
        result
    }

    /// Get all distinct items from loot history.
    /// Returns (item_id, item_name) pairs sorted by name for autocomplete.
    ///
    /// Older rows can have a raw unresolved localization key stored as the
    /// name (e.g. `{textiles.Large_Sanctuary_Cloth}`) if it was recorded
    /// before the item was resolvable in the local asset data. Those are
    /// re-resolved live via the asset manager, falling back to humanizing the
    /// stored key itself if the item still can't be resolved.
    pub fn get_distinct_items(&self) -> SqlResult<Vec<(i32, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT item_id, item_name FROM loot_items ORDER BY item_name ASC")?;
        let result: Vec<(i32, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<SqlResult<Vec<_>>>()?;
        Ok(result
            .into_iter()
            .map(|(item_id, name)| (item_id, Self::resolve_display_name(item_id, name)))
            .collect())
    }

    /// Resolve the best display name for a stored (item_id, name) pair,
    /// re-resolving an unresolved localization key via the asset manager (or
    /// humanizing it directly if the item isn't in local asset data).
    fn resolve_display_name(item_id: i32, name: String) -> String {
        if !(name.starts_with('{') && name.ends_with('}')) {
            return name;
        }
        if let Some(resolved) = get_asset_manager().object_name(item_id) {
            if !(resolved.starts_with('{') && resolved.ends_with('}')) {
                return resolved;
            }
        }
        // Fall back to humanizing the key itself, e.g.
        // "{textiles.Large_Sanctuary_Cloth}" -> "Large Sanctuary Cloth".
        name.trim_start_matches('{')
            .trim_end_matches('}')
            .rsplit('.')
            .next()
            .unwrap_or(&name)
            .replace('_', " ")
    }

    /// Delete all drops (for testing/reset).
    pub fn clear_all(&mut self) -> SqlResult<()> {
        self.conn.execute("DELETE FROM loot_items", [])?;
        self.conn.execute("DELETE FROM loot_drops", [])?;
        Ok(())
    }

    /// Delete the given loot drops by id, along with their `loot_items` rows.
    /// Returns the number of drop rows deleted.
    pub fn delete_drops(&mut self, ids: &[i64]) -> SqlResult<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.transaction()?;
        let mut deleted = 0usize;
        {
            let mut del_items = tx.prepare("DELETE FROM loot_items WHERE drop_id = ?1")?;
            let mut del_drop = tx.prepare("DELETE FROM loot_drops WHERE id = ?1")?;
            for &id in ids {
                del_items.execute([id])?;
                deleted += del_drop.execute([id])?;
            }
        }
        tx.commit()?;
        Ok(deleted)
    }

    // ========== Dungeon Stats Aggregation Queries (read-only) ==========

    /// Get the highest drop ID (for cache invalidation).
    pub fn last_drop_id(&self) -> SqlResult<Option<i64>> {
        self.conn
            .query_row("SELECT MAX(id) FROM loot_drops", [], |row| row.get(0))
    }

    /// Count dungeon completions based on boss drops.
    /// A completion = a distinct (dungeon, map_seed, mob_type) tuple where mob_type is a known boss.
    /// Using map_seed ensures all bags from the same dungeon run are grouped,
    /// and mob_type dedup handles overflow bags from the same boss.
    pub fn completions_by_dungeon(
        &self,
        boss_ids: &std::collections::HashSet<i32>,
    ) -> SqlResult<std::collections::HashMap<String, i32>> {
        if boss_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        let placeholders: String = boss_ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",");

        let sql = format!(
            "SELECT dungeon, COUNT(*) as completions FROM (
                SELECT DISTINCT dungeon, map_seed, mob_type
                FROM loot_drops
                WHERE mob_type IN ({})
            ) GROUP BY dungeon",
            placeholders
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i32>(1)?))
        })?;

        let mut result = std::collections::HashMap::new();
        for row in rows {
            let (dungeon, count) = row?;
            result.insert(dungeon, count);
        }
        Ok(result)
    }

    /// Count dungeon completions by counting mark drops per dungeon.
    /// Each mark drop = 1 completion. Falls back to boss-kill counting if no marks found.
    pub fn mark_completions_by_dungeon(
        &self,
        mark_item_ids: &std::collections::HashSet<i32>,
    ) -> SqlResult<std::collections::HashMap<String, i32>> {
        if mark_item_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        let placeholders: String = mark_item_ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",");

        let sql = format!(
            "SELECT d.dungeon, COUNT(*) as completions
             FROM loot_items i
             JOIN loot_drops d ON i.drop_id = d.id
             WHERE i.item_id IN ({})
             GROUP BY d.dungeon",
            placeholders
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i32>(1)?))
        })?;

        let mut result = std::collections::HashMap::new();
        for row in rows {
            let (dungeon, count) = row?;
            result.insert(dungeon, count);
        }
        Ok(result)
    }

    /// Get item statistics for a specific dungeon.
    /// Groups by item, counts occurrences, and collects bag type distribution.
    pub fn items_by_dungeon(&self, dungeon: &str) -> SqlResult<Vec<DungeonItemStat>> {
        let mut stmt = self.conn.prepare(
            "SELECT i.item_id, i.item_name, COUNT(*) as cnt, d.bag_type
             FROM loot_items i
             JOIN loot_drops d ON i.drop_id = d.id
             WHERE d.dungeon = ?1
             GROUP BY i.item_id, i.item_name, d.bag_type
             ORDER BY cnt DESC",
        )?;

        // Collect raw rows grouped by (item_id, item_name)
        let mut items: std::collections::HashMap<(i32, String), DungeonItemStat> =
            std::collections::HashMap::new();

        let rows = stmt.query_map([dungeon], |row| {
            Ok((
                row.get::<_, i32>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i32>(3)?,
            ))
        })?;

        for row in rows {
            let (item_id, item_name, count, bag_type) = row?;
            let entry = items
                .entry((item_id, item_name.clone()))
                .or_insert_with(|| DungeonItemStat {
                    item_id,
                    item_name,
                    total_count: 0,
                    bag_type_counts: Vec::new(),
                });
            entry.total_count += count;
            entry.bag_type_counts.push((bag_type, count));
        }

        let mut result: Vec<DungeonItemStat> = items.into_values().collect();
        result.sort_by(|a, b| b.total_count.cmp(&a.total_count));
        Ok(result)
    }

    /// Get item statistics per mob for a specific dungeon.
    pub fn items_by_dungeon_and_mob(&self, dungeon: &str) -> SqlResult<Vec<MobItemStat>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.mob_type, d.mob_name, i.item_id, i.item_name, COUNT(*) as cnt
             FROM loot_items i
             JOIN loot_drops d ON i.drop_id = d.id
             WHERE d.dungeon = ?1
             GROUP BY d.mob_type, d.mob_name, i.item_id, i.item_name
             ORDER BY d.mob_name ASC, cnt DESC",
        )?;

        let result = stmt
            .query_map([dungeon], |row| {
                Ok(MobItemStat {
                    mob_type: row.get(0)?,
                    mob_name: row.get(1)?,
                    item_id: row.get(2)?,
                    item_name: row.get(3)?,
                    count: row.get(4)?,
                })
            })?
            .collect();
        result
    }

    /// Batch query: get rarity breakdowns for all items in the given dungeons.
    /// Returns a map of item_id -> Vec<(enchant_count, drop_count)>.
    pub fn all_rarity_breakdowns(
        &self,
        dungeons: &[String],
    ) -> SqlResult<std::collections::HashMap<i32, Vec<(i32, i64)>>> {
        use std::collections::HashMap;
        if dungeons.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders: String = dungeons.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT i.item_id, i.enchant_count, COUNT(*) as cnt
             FROM loot_items i
             JOIN loot_drops d ON i.drop_id = d.id
             WHERE d.dungeon IN ({})
             GROUP BY i.item_id, i.enchant_count
             ORDER BY i.item_id, i.enchant_count ASC",
            placeholders
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let params: Vec<Box<dyn rusqlite::types::ToSql>> = dungeons
            .iter()
            .map(|d| Box::new(d.clone()) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            Ok((
                row.get::<_, i32>(0)?,
                row.get::<_, i32>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;

        let mut map: HashMap<i32, Vec<(i32, i64)>> = HashMap::new();
        for row in rows {
            let (item_id, enchant_count, count) = row?;
            map.entry(item_id).or_default().push((enchant_count, count));
        }
        Ok(map)
    }
}

/// Item statistics for a dungeon (aggregated across all drops).
#[derive(Debug, Clone)]
pub struct DungeonItemStat {
    pub item_id: i32,
    pub item_name: String,
    pub total_count: i64,
    /// (bag_type_id, count) pairs showing which bag types this item dropped in
    pub bag_type_counts: Vec<(i32, i64)>,
}

/// Per-mob item statistics within a dungeon.
#[derive(Debug, Clone)]
pub struct MobItemStat {
    pub mob_type: i32,
    pub mob_name: String,
    pub item_id: i32,
    pub item_name: String,
    pub count: i64,
}

/// Loot statistics summary.
#[derive(Debug, Clone)]
pub struct LootStatistics {
    /// Total number of drops
    pub total_drops: i64,
    /// Total white bags
    pub white_bags: i64,
    /// Total orange bags  
    pub orange_bags: i64,
    /// Total red bags
    pub red_bags: i64,
    /// Count by bag type
    pub by_bag_type: Vec<(i32, i64)>,
    /// Count by dungeon
    pub by_dungeon: Vec<(String, i64)>,
}

impl LootDropRecord {
    /// Get the drop timestamp as DateTime.
    pub fn timestamp_utc(&self) -> DateTime<Utc> {
        Utc.timestamp_millis_opt(self.timestamp).unwrap()
    }

    /// Get bag type enum.
    pub fn bag_type_enum(&self) -> Option<LootBagType> {
        LootBagType::from_id(self.bag_type)
    }
}

impl LootItemRecord {
    /// Get enchant IDs (returns pre-parsed list).
    pub fn enchant_id_list(&self) -> &[i32] {
        &self.parsed_enchant_ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_player() -> PlayerSnapshot {
        PlayerSnapshot {
            char_id: 1234,
            object_id: 5678,
            class_id: 768,
            name: "TestPlayer".to_string(),
            skin_id: 0,
            tex1: 0,
            tex2: 0,
            fame: 50000,
            is_seasonal: false,
            exalt_bonus: 10,
            loot_drop_active: true,
            loot_tier_active: false,
            dungeon: "The Void".to_string(),
            map_seed: 99999,
        }
    }

    #[test]
    fn test_open_in_memory() {
        let db = LootDatabase::open_in_memory();
        assert!(db.is_ok());
    }

    #[test]
    fn writer_enables_wal_busy_timeout_and_user_version() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("loot_history.db");
        let db = LootDatabase::open_writer(&path, None).unwrap();

        let journal_mode: String = db
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
        let busy_timeout: i64 = db
            .conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert_eq!(busy_timeout, 5000);
        let user_version: i32 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(user_version, SCHEMA_VERSION);
    }

    #[test]
    fn reader_is_read_only_and_does_not_migrate() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("loot_history.db");
        // Writer creates the schema first, before the UI reader opens.
        let _writer = LootDatabase::open_writer(&path, None).unwrap();
        let reader = LootDatabase::open_reader_at(&path).unwrap();
        // A read-only connection rejects writes.
        assert!(reader
            .conn
            .execute("CREATE TABLE should_fail (x INTEGER)", [])
            .is_err());
    }

    #[test]
    fn two_loot_profiles_isolate_identical_row_identifiers() {
        let temp = tempfile::tempdir().unwrap();
        let a_path = temp.path().join("a").join("loot_history.db");
        let b_path = temp.path().join("b").join("loot_history.db");
        let mut a = LootDatabase::open_writer(&a_path, None).unwrap();
        let mut b = LootDatabase::open_writer(&b_path, None).unwrap();

        // Identical logical identifiers (timestamp, map_seed via player), but
        // distinct item payloads.
        let drop_a = NewLootDrop {
            timestamp: 1700000000000,
            player: create_test_player(),
            bag_type: LootBagType::White,
            mob_type: 100,
            mob_name: "Boss".to_string(),
            items: vec![NewLootItem {
                slot: 0,
                item_id: 111,
                item_name: "Profile A Item".to_string(),
                enchant_ids: vec![],
            }],
        };
        let drop_b = NewLootDrop {
            items: vec![NewLootItem {
                slot: 0,
                item_id: 222,
                item_name: "Profile B Item".to_string(),
                enchant_ids: vec![],
            }],
            ..drop_a.clone()
        };
        let id_a = a.insert_loot_drop(&drop_a).unwrap();
        let id_b = b.insert_loot_drop(&drop_b).unwrap();
        assert_eq!(id_a, id_b, "each profile starts its own row id space");

        assert_eq!(a.total_drops().unwrap(), 1);
        assert_eq!(b.total_drops().unwrap(), 1);
        let items_a = &a.get_recent_drops(10).unwrap()[0].items;
        let items_b = &b.get_recent_drops(10).unwrap()[0].items;
        assert_eq!(items_a[0].item_id, 111);
        assert_eq!(items_b[0].item_id, 222);
    }

    #[test]
    fn backfill_reads_injected_combat_path_and_isolates_profiles() {
        let temp = tempfile::tempdir().unwrap();
        let combat_path = temp.path().join("a").join("combat_history.db");
        std::fs::create_dir_all(combat_path.parent().unwrap()).unwrap();
        {
            let conn = Connection::open(&combat_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE fights (map_seed INTEGER, boss_object_type INTEGER, boss_name TEXT, started_at INTEGER, ended_at INTEGER, killed INTEGER);
                 INSERT INTO fights VALUES (777, 5555, 'Test Boss', 100, 200, 1);
                 INSERT INTO fights VALUES (0, 0, 'not-a-kill', 0, 0, 0);",
            )
            .unwrap();
        }

        // A loot profile paired with this combat path reads its recorded kills.
        let loot_path = temp.path().join("a").join("loot_history.db");
        let paired = LootDatabase::open_writer(&loot_path, Some(&combat_path)).unwrap();
        let (kills, readable) = paired.load_recorded_boss_kills();
        assert!(readable);
        assert_eq!(kills.len(), 1);
        assert_eq!(kills[0].object_type, 5555);
        assert_eq!(kills[0].map_seed, 777);

        // A different profile paired with its own (missing) combat path sees none.
        let other_combat = temp.path().join("b").join("combat_history.db");
        let other_loot = temp.path().join("b").join("loot_history.db");
        let unpaired = LootDatabase::open_writer(&other_loot, Some(&other_combat)).unwrap();
        let (kills_b, readable_b) = unpaired.load_recorded_boss_kills();
        assert!(readable_b);
        assert!(kills_b.is_empty());
    }

    #[test]
    fn test_insert_and_query() {
        let mut db = LootDatabase::open_in_memory().unwrap();

        let drop = NewLootDrop {
            timestamp: 1700000000000,
            player: create_test_player(),
            bag_type: LootBagType::White,
            mob_type: 45076,
            mob_name: "Void Entity".to_string(),
            items: vec![NewLootItem {
                slot: 0,
                item_id: 9000,
                item_name: "Void Blade".to_string(),
                enchant_ids: vec![],
            }],
        };

        let id = db.insert_loot_drop(&drop).unwrap();
        assert!(id > 0);

        let drops = db.get_recent_drops(10).unwrap();
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].char_name, "TestPlayer");
        assert_eq!(drops[0].mob_name, "Void Entity");
        assert_eq!(drops[0].items.len(), 1);
        assert_eq!(drops[0].items[0].item_name, "Void Blade");
    }

    #[test]
    fn migration_v13_backfills_unknown_via_fight_correlation_path() {
        let mut db = LootDatabase::open_in_memory().unwrap();

        // A non-curated-dungeon bag stored Unknown that in fact holds Mark of
        // Janus (Janus emits loot anonymously). The v13 pass runs the live
        // override rules over the recorded fights and recovers it. This also
        // proves the backfill only runs when recorded kills exist.
        let player = create_test_player(); // dungeon: "The Void" (non-curated)
        let id = db
            .insert_loot_drop(&NewLootDrop {
                timestamp: 1_700_000_000_000,
                player,
                bag_type: LootBagType::White,
                mob_type: 0,
                mob_name: "Unknown".to_string(),
                items: vec![NewLootItem {
                    slot: 0,
                    item_id: super::super::MARK_OF_JANUS_ITEM_ID,
                    item_name: "Mark of Janus".to_string(),
                    enchant_ids: vec![],
                }],
            })
            .unwrap();

        // Some recorded kill must exist for the backfill to run at all.
        let kills = vec![crate::loot::RecentBossKill {
            map_seed: 99999,
            object_type: 20493,
            name: "Some Boss".to_string(),
            started_at_ms: 1_699_999_900_000,
            ended_at_ms: 1_699_999_950_000,
        }];

        db.backfill_unknown_with_kills(&kills).unwrap();

        let drop = db
            .get_recent_drops(10)
            .unwrap()
            .into_iter()
            .find(|d| d.id == id)
            .unwrap();
        assert_eq!(drop.mob_type, super::super::JANUS_OBJECT_TYPE);
        assert_eq!(drop.mob_name, super::super::JANUS_NAME);
    }

    #[test]
    fn migration_v13_leaves_non_boss_bag_unknown() {
        let mut db = LootDatabase::open_in_memory().unwrap();

        // A non-curated-dungeon bag holding no boss-only loot must stay Unknown:
        // there is nothing a fight can attribute it to.
        let player = create_test_player(); // dungeon: "The Void" (non-curated)
        let id = db
            .insert_loot_drop(&NewLootDrop {
                timestamp: 1_700_000_000_000,
                player,
                bag_type: LootBagType::White,
                mob_type: 0,
                mob_name: "Unknown".to_string(),
                items: vec![NewLootItem {
                    slot: 0,
                    item_id: 9001,
                    item_name: "Random Trinket".to_string(),
                    enchant_ids: vec![],
                }],
            })
            .unwrap();

        let kills = vec![crate::loot::RecentBossKill {
            map_seed: 99999,
            object_type: 20493,
            name: "Some Boss".to_string(),
            started_at_ms: 1_699_999_900_000,
            ended_at_ms: 1_699_999_950_000,
        }];

        db.backfill_unknown_with_kills(&kills).unwrap();

        let drop = db
            .get_recent_drops(10)
            .unwrap()
            .into_iter()
            .find(|d| d.id == id)
            .unwrap();
        assert_eq!(drop.mob_type, 0);
        assert_eq!(drop.mob_name, "Unknown");
    }

    #[test]
    fn migration_backfill_attributes_unknown_beehemoth_bags_by_closest_kill() {
        let mut db = LootDatabase::open_in_memory().unwrap();

        // Three colourless Killer Bee Nest event bags stored Unknown in the realm
        // (the invisible EH dropper is never hit, so proximity can't tag them).
        // Each dropped ~1s after its Beehemoth died; the backfill must map each
        // bag to its own bee by the death closest in time.
        let seed = 1114580691;
        let mut mk = |ts: i64| {
            let mut player = create_test_player();
            player.dungeon = "{s.rotmg}".to_string();
            player.map_seed = seed;
            db.insert_loot_drop(&NewLootDrop {
                timestamp: ts,
                player,
                bag_type: LootBagType::BoostedBlue,
                mob_type: 0,
                mob_name: "Unknown".to_string(),
                items: vec![NewLootItem {
                    slot: 0,
                    item_id: 9999,
                    item_name: "Generic Loot".to_string(),
                    enchant_ids: vec![],
                }],
            })
            .unwrap()
        };
        let red_bag = mk(1787682792350);
        let yellow_bag = mk(1787682807216);
        let blue_bag = mk(1787682829239);

        let kills = vec![
            crate::loot::RecentBossKill {
                map_seed: seed,
                object_type: super::super::RED_BEEHEMOTH_OBJECT_TYPE,
                name: "Red Beehemoth".to_string(),
                started_at_ms: 1787682763865,
                ended_at_ms: 1787682791357,
            },
            crate::loot::RecentBossKill {
                map_seed: seed,
                object_type: super::super::YELLOW_BEEHEMOTH_OBJECT_TYPE,
                name: "Yellow Beehemoth".to_string(),
                started_at_ms: 1787682764515,
                ended_at_ms: 1787682806239,
            },
            crate::loot::RecentBossKill {
                map_seed: seed,
                object_type: super::super::BLUE_BEEHEMOTH_OBJECT_TYPE,
                name: "Blue Beehemoth".to_string(),
                started_at_ms: 1787682763651,
                ended_at_ms: 1787682828255,
            },
        ];

        db.backfill_unknown_with_kills(&kills).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        let find = |id: i64| drops.iter().find(|d| d.id == id).unwrap();
        assert_eq!(
            find(red_bag).mob_type,
            super::super::RED_BEEHEMOTH_OBJECT_TYPE
        );
        assert_eq!(
            find(yellow_bag).mob_type,
            super::super::YELLOW_BEEHEMOTH_OBJECT_TYPE
        );
        assert_eq!(
            find(blue_bag).mob_type,
            super::super::BLUE_BEEHEMOTH_OBJECT_TYPE
        );
    }

    #[test]
    fn migration_v13_skips_curated_dungeons() {
        let mut db = LootDatabase::open_in_memory().unwrap();

        // Spectral Penitentiary has its own conservative per-run backfill that
        // may deliberately leave a bag Unknown; the generic fight-correlation
        // pass must not override it, even with a matching recorded kill.
        let mut player = create_test_player();
        player.dungeon = crate::loot::SPECTRAL_PENITENTIARY_NAME.to_string();
        player.map_seed = 4242;

        let id = db
            .insert_loot_drop(&NewLootDrop {
                timestamp: 1_700_000_000_000,
                player,
                bag_type: LootBagType::White,
                mob_type: 0,
                mob_name: "Unknown".to_string(),
                items: vec![NewLootItem {
                    slot: 0,
                    item_id: 9002,
                    item_name: "Trinket".to_string(),
                    enchant_ids: vec![],
                }],
            })
            .unwrap();

        let kills = vec![crate::loot::RecentBossKill {
            map_seed: 4242,
            object_type: super::super::SPEN_OCULON_OBJECT_TYPE,
            name: "Overseer Oculon".to_string(),
            started_at_ms: 1_699_999_900_000,
            ended_at_ms: 1_699_999_950_000,
        }];

        db.backfill_unknown_with_kills(&kills).unwrap();

        let drop = db
            .get_recent_drops(10)
            .unwrap()
            .into_iter()
            .find(|d| d.id == id)
            .unwrap();
        assert_eq!(drop.mob_type, 0);
        assert_eq!(drop.mob_name, "Unknown");
    }

    // --- Killer Bee Nest realm event migration (v21) ---

    fn insert_realm_bag(
        db: &mut LootDatabase,
        mob_type: i32,
        map_seed: i32,
        items: &[(i32, &str)],
    ) -> i64 {
        let mut player = create_test_player();
        player.dungeon = "{s.rotmg}".to_string();
        player.map_seed = map_seed;
        db.insert_loot_drop(&NewLootDrop {
            timestamp: 1_700_000_000_000,
            player,
            bag_type: LootBagType::White,
            mob_type,
            mob_name: if mob_type == 0 {
                "Unknown".to_string()
            } else {
                "EH Event Taunt Controller".to_string()
            },
            items: items
                .iter()
                .enumerate()
                .map(|(slot, (id, name))| NewLootItem {
                    slot: slot as i32,
                    item_id: *id,
                    item_name: name.to_string(),
                    enchant_ids: vec![],
                })
                .collect(),
        })
        .unwrap()
    }

    fn drop_source(db: &LootDatabase, id: i64) -> (i32, String) {
        let d = db
            .get_recent_drops(50)
            .unwrap()
            .into_iter()
            .find(|d| d.id == id)
            .unwrap();
        (d.mob_type, d.mob_name)
    }

    #[test]
    fn migration_v21_single_color_from_taunt_controller() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        let id = insert_realm_bag(
            &mut db,
            super::super::EH_EVENT_TAUNT_CONTROLLER_OBJECT_TYPE,
            42,
            &[(4310, "Beehemoth Armor")], // Blue armor
        );
        db.backfill_killer_bee_nest().unwrap();
        assert_eq!(
            drop_source(&db, id),
            (
                super::super::BLUE_BEEHEMOTH_OBJECT_TYPE,
                "Blue Beehemoth".to_string()
            )
        );
    }

    #[test]
    fn migration_v21_single_color_from_unknown() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        let id = insert_realm_bag(&mut db, 0, 42, &[(4339, "Beehemoth Quiver")]); // Red quiver
        db.backfill_killer_bee_nest().unwrap();
        assert_eq!(
            drop_source(&db, id),
            (
                super::super::RED_BEEHEMOTH_OBJECT_TYPE,
                "Red Beehemoth".to_string()
            )
        );
    }

    #[test]
    fn migration_v21_mixed_colors_to_bramblethorn() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        let id = insert_realm_bag(
            &mut db,
            super::super::EH_EVENT_TAUNT_CONTROLLER_OBJECT_TYPE,
            42,
            &[(4338, "Beehemoth Quiver"), (4309, "Beehemoth Armor")], // Blue + Red
        );
        db.backfill_killer_bee_nest().unwrap();
        assert_eq!(
            drop_source(&db, id),
            (
                super::super::CORRUPTED_BRAMBLETHORN_OBJECT_TYPE,
                super::super::CORRUPTED_BRAMBLETHORN_NAME.to_string()
            )
        );
    }

    #[test]
    fn migration_v21_colorless_taunt_controller_defaults_blue() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        let id = insert_realm_bag(
            &mut db,
            super::super::EH_EVENT_TAUNT_CONTROLLER_OBJECT_TYPE,
            42,
            &[(9999, "Random Trinket")],
        );
        db.backfill_killer_bee_nest().unwrap();
        assert_eq!(
            drop_source(&db, id),
            (
                super::super::BLUE_BEEHEMOTH_OBJECT_TYPE,
                "Blue Beehemoth".to_string()
            )
        );
    }

    #[test]
    fn migration_v21_colorless_unknown_left_alone() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        let id = insert_realm_bag(&mut db, 0, 42, &[(9999, "Random Trinket")]);
        db.backfill_killer_bee_nest().unwrap();
        assert_eq!(drop_source(&db, id), (0, "Unknown".to_string()));
    }

    #[test]
    fn migration_v21_leaves_real_boss_bag_untouched() {
        // A bag already attributed to Corrupted Bramblethorn is out of scope
        // (mob_type not 0 or the taunt controller) and stays as-is.
        let mut db = LootDatabase::open_in_memory().unwrap();
        let id = insert_realm_bag(
            &mut db,
            super::super::CORRUPTED_BRAMBLETHORN_OBJECT_TYPE,
            42,
            &[(4338, "Beehemoth Quiver")],
        );
        db.backfill_killer_bee_nest().unwrap();
        assert_eq!(
            drop_source(&db, id),
            (
                super::super::CORRUPTED_BRAMBLETHORN_OBJECT_TYPE,
                "EH Event Taunt Controller".to_string()
            )
        );
    }

    #[test]
    fn migration_v22_realm_event_source_from_unknown_loot() {
        // A realm bag whose rare loot (Token of Happiness) points to a single
        // realm event (the Ethereal Shrine) is attributed to that event.
        let mgr = crate::assets::get_asset_manager();
        if let Some(dir) = crate::assets::find_assets_dir() {
            mgr.set_assets_dir(&dir);
        }
        let shrine = mgr.object_id_for_display_name("Ethereal Shrine");
        if !mgr.try_load() || shrine.is_none() {
            eprintln!("skipping: game assets not available");
            return;
        }
        let shrine = shrine.unwrap();

        let mut db = LootDatabase::open_in_memory().unwrap();
        // 20764 = Token of Happiness (Ethereal Shrine, non-seasonal biomes).
        let id = insert_realm_bag(&mut db, 0, 77, &[(20764, "Token of Happiness")]);
        db.migrate(21).unwrap();

        assert_eq!(drop_source(&db, id).0, shrine);
    }

    #[test]
    fn migration_v22_ambiguous_loot_stays_unknown() {
        // Seal of Blasphemous Prayer drops from several bosses, so its realm
        // source is ambiguous and the bag is left Unknown.
        let mgr = crate::assets::get_asset_manager();
        if let Some(dir) = crate::assets::find_assets_dir() {
            mgr.set_assets_dir(&dir);
        }
        if !mgr.try_load() || mgr.object_id_for_display_name("Ethereal Shrine").is_none() {
            eprintln!("skipping: game assets not available");
            return;
        }

        let mut db = LootDatabase::open_in_memory().unwrap();
        // 3078 = Seal of Blasphemous Prayer (multiple sources).
        let id = insert_realm_bag(&mut db, 0, 77, &[(3078, "Seal of Blasphemous Prayer")]);
        db.migrate(21).unwrap();

        assert_eq!(drop_source(&db, id), (0, "Unknown".to_string()));
    }

    #[test]
    fn migration_v22_mark_consensus_backfills_unknown() {
        // Legacy Unknown bags holding a boss Mark are attributed to that
        // dungeon's main boss by consensus of already-attributed Mark bags.
        let mgr = crate::assets::get_asset_manager();
        if let Some(dir) = crate::assets::find_assets_dir() {
            mgr.set_assets_dir(&dir);
        }
        let sep = mgr.object_id_for_name("Septavius the Ghost God");
        // 7718 = Mark of Septavius.
        if !mgr.try_load() || sep.is_none() || !mgr.boss_mark_item_ids().contains(&7718) {
            eprintln!("skipping: game assets not available");
            return;
        }
        let sep = sep.unwrap();

        let insert = |db: &mut LootDatabase, mob_type: i32, name: &str| -> i64 {
            let mut player = create_test_player();
            player.dungeon = "The Snake Pit".to_string();
            player.map_seed = 55;
            db.insert_loot_drop(&NewLootDrop {
                timestamp: 1_700_000_000_000,
                player,
                bag_type: LootBagType::White,
                mob_type,
                mob_name: name.to_string(),
                items: vec![NewLootItem {
                    slot: 0,
                    item_id: 7718,
                    item_name: "Mark of Septavius".to_string(),
                    enchant_ids: vec![],
                }],
            })
            .unwrap()
        };

        let mut db = LootDatabase::open_in_memory().unwrap();
        insert(&mut db, sep, "Septavius the Ghost God");
        insert(&mut db, sep, "Septavius the Ghost God");
        let unknown = insert(&mut db, 0, "Unknown");

        db.migrate(21).unwrap();

        assert_eq!(
            drop_source(&db, unknown),
            (sep, "Septavius the Ghost God".to_string())
        );
    }

    #[test]
    fn migration_v22_mark_no_consensus_left_unknown() {
        // Without any attributed Mark bag to vote, an Unknown Mark bag is left
        // Unknown (legacy runs predate combat tracking; no kill to correlate).
        let mgr = crate::assets::get_asset_manager();
        if let Some(dir) = crate::assets::find_assets_dir() {
            mgr.set_assets_dir(&dir);
        }
        if !mgr.try_load() || !mgr.boss_mark_item_ids().contains(&7718) {
            eprintln!("skipping: game assets not available");
            return;
        }

        let mut db = LootDatabase::open_in_memory().unwrap();
        let mut player = create_test_player();
        player.dungeon = "The Snake Pit".to_string();
        player.map_seed = 55;
        let unknown = db
            .insert_loot_drop(&NewLootDrop {
                timestamp: 1_700_000_000_000,
                player,
                bag_type: LootBagType::White,
                mob_type: 0,
                mob_name: "Unknown".to_string(),
                items: vec![NewLootItem {
                    slot: 0,
                    item_id: 7718,
                    item_name: "Mark of Septavius".to_string(),
                    enchant_ids: vec![],
                }],
            })
            .unwrap();

        db.migrate(21).unwrap();

        assert_eq!(drop_source(&db, unknown), (0, "Unknown".to_string()));
    }

    #[test]
    #[ignore]
    fn validate_v22_against_real_db_copy() {
        // Dry-run harness: set RH_REAL_DB to a *copy* of loot_history.db, then
        // `cargo test -p realmhound-core validate_v22_against_real_db_copy -- --ignored --nocapture`.
        let Ok(path) = std::env::var("RH_REAL_DB") else {
            eprintln!("RH_REAL_DB not set; skipping");
            return;
        };
        let mgr = crate::assets::get_asset_manager();
        if let Some(dir) = crate::assets::find_assets_dir() {
            mgr.set_assets_dir(&dir);
        }
        mgr.try_load();

        let unknown_sql = "SELECT COUNT(*) FROM loot_drops WHERE mob_type = 0";
        // Count Unknown bags before migrating (raw connection, no migration).
        let before: i64 = {
            let conn = Connection::open(&path).unwrap();
            conn.query_row(unknown_sql, [], |r| r.get(0)).unwrap()
        };
        // open_writer runs the v22 migration.
        let db = LootDatabase::open_writer(std::path::Path::new(&path), None).unwrap();
        let after: i64 = db.conn.query_row(unknown_sql, [], |r| r.get(0)).unwrap();
        eprintln!(
            "Unknown bags: {before} -> {after} (fixed {})",
            before - after
        );
    }

    #[test]
    fn migration_v4_backfills_janus_from_mark() {
        let mut db = LootDatabase::open_in_memory().unwrap();

        // Legacy bag stored as Unknown that actually holds Mark of Janus.
        let janus = db
            .insert_loot_drop(&NewLootDrop {
                timestamp: 1700000000000,
                player: create_test_player(),
                bag_type: LootBagType::Purple,
                mob_type: 0,
                mob_name: "Unknown".to_string(),
                items: vec![NewLootItem {
                    slot: 0,
                    item_id: super::super::MARK_OF_JANUS_ITEM_ID,
                    item_name: "Mark of Janus".to_string(),
                    enchant_ids: vec![],
                }],
            })
            .unwrap();

        // Public bag with no Mark stays Unknown.
        let public = db
            .insert_loot_drop(&NewLootDrop {
                timestamp: 1700000001000,
                player: create_test_player(),
                bag_type: LootBagType::Brown,
                mob_type: 0,
                mob_name: "Unknown".to_string(),
                items: vec![NewLootItem {
                    slot: 0,
                    item_id: 9000,
                    item_name: "Some Item".to_string(),
                    enchant_ids: vec![],
                }],
            })
            .unwrap();

        db.migrate(3).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        let janus_row = drops.iter().find(|d| d.id == janus).unwrap();
        let public_row = drops.iter().find(|d| d.id == public).unwrap();

        assert_eq!(janus_row.mob_type, super::super::JANUS_OBJECT_TYPE);
        assert_eq!(janus_row.mob_name, super::super::JANUS_NAME);
        assert_eq!(public_row.mob_type, 0);
        assert_eq!(public_row.mob_name, "Unknown");
    }

    #[test]
    fn migration_v18_attributes_oryx_sanctuary_beisa() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        let mut player = create_test_player();
        player.dungeon = super::super::ORYX_SANCTUARY_NAME.to_string();

        // Unknown O3 bag with a Greater Potion of Life -> Chief Beisa.
        let pot_bag = db
            .insert_loot_drop(&NewLootDrop {
                timestamp: 1700000000000,
                player: player.clone(),
                bag_type: LootBagType::Blue,
                mob_type: 0,
                mob_name: "Unknown".to_string(),
                items: vec![NewLootItem {
                    slot: 0,
                    item_id: super::super::GREATER_POTION_OF_LIFE_ITEM_ID,
                    item_name: "Greater Potion of Life".to_string(),
                    enchant_ids: vec![],
                }],
            })
            .unwrap();

        // Unknown O3 bag with a Beisa drop-table item (5146) -> Chief Beisa.
        let table_bag = db
            .insert_loot_drop(&NewLootDrop {
                timestamp: 1700000001000,
                player: player.clone(),
                bag_type: LootBagType::Red,
                mob_type: 0,
                mob_name: "Unknown".to_string(),
                items: vec![NewLootItem {
                    slot: 0,
                    item_id: 5146,
                    item_name: "Beisa Drop".to_string(),
                    enchant_ids: vec![],
                }],
            })
            .unwrap();

        // Unknown O3 bag with an unrelated item and no potion -> stays Unknown.
        let other_bag = db
            .insert_loot_drop(&NewLootDrop {
                timestamp: 1700000002000,
                player: player.clone(),
                bag_type: LootBagType::Purple,
                mob_type: 0,
                mob_name: "Unknown".to_string(),
                items: vec![NewLootItem {
                    slot: 0,
                    item_id: 17840,
                    item_name: "Demon Blade".to_string(),
                    enchant_ids: vec![],
                }],
            })
            .unwrap();

        // Brown (public) O3 bag with a potion -> never attributed to a mob.
        let brown_bag = db
            .insert_loot_drop(&NewLootDrop {
                timestamp: 1700000003000,
                player: player.clone(),
                bag_type: LootBagType::Brown,
                mob_type: 0,
                mob_name: "Unknown".to_string(),
                items: vec![NewLootItem {
                    slot: 0,
                    item_id: super::super::GREATER_POTION_OF_MANA_ITEM_ID,
                    item_name: "Greater Potion of Mana".to_string(),
                    enchant_ids: vec![],
                }],
            })
            .unwrap();

        db.migrate(17).unwrap();

        let drops = db.get_recent_drops(20).unwrap();
        let row = |id: i64| drops.iter().find(|d| d.id == id).unwrap();

        assert_eq!(row(pot_bag).mob_type, super::super::CHIEF_BEISA_OBJECT_TYPE);
        assert_eq!(row(pot_bag).mob_name, super::super::CHIEF_BEISA_NAME);
        assert_eq!(
            row(table_bag).mob_type,
            super::super::CHIEF_BEISA_OBJECT_TYPE
        );
        assert_eq!(row(other_bag).mob_type, 0);
        assert_eq!(row(brown_bag).mob_type, 0);
    }

    #[test]
    fn migration_v19_repairs_beisa_when_v18_was_consumed_empty() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        let mut player = create_test_player();
        player.dungeon = super::super::ORYX_SANCTUARY_NAME.to_string();

        // An install stuck at v18 (an intermediate build bumped the version
        // before the relabel shipped) still has this Unknown O3 potion bag.
        let pot_bag = db
            .insert_loot_drop(&NewLootDrop {
                timestamp: 1700000000000,
                player: player.clone(),
                bag_type: LootBagType::Blue,
                mob_type: 0,
                mob_name: "Unknown".to_string(),
                items: vec![NewLootItem {
                    slot: 0,
                    item_id: super::super::GREATER_POTION_OF_MANA_ITEM_ID,
                    item_name: "Greater Potion of Mana".to_string(),
                    enchant_ids: vec![],
                }],
            })
            .unwrap();

        // Migrating from v18 (not v17) must still repair it via the v19 gate.
        db.migrate(18).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        let row = drops.iter().find(|d| d.id == pot_bag).unwrap();
        assert_eq!(row.mob_type, super::super::CHIEF_BEISA_OBJECT_TYPE);
        assert_eq!(row.mob_name, super::super::CHIEF_BEISA_NAME);
    }

    #[test]
    fn migration_v5_backfills_lair_of_draconis_dragons() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        let mut player = create_test_player();
        player.dungeon = super::super::LAIR_OF_DRACONIS_NAME.to_string();

        // Color greater potion identifies the dragon (primary signal).
        let pyyr = db
            .insert_loot_drop(&NewLootDrop {
                timestamp: 1700000000000,
                player: player.clone(),
                bag_type: LootBagType::Purple,
                mob_type: 0,
                mob_name: "Unknown".to_string(),
                items: vec![NewLootItem {
                    slot: 0,
                    item_id: 9067,
                    item_name: "Greater Potion of Vitality".to_string(),
                    enchant_ids: vec![],
                }],
            })
            .unwrap();

        // No potion, but a per-dragon UT/ST identifies it (fallback signal).
        let nikao = db
            .insert_loot_drop(&NewLootDrop {
                timestamp: 1700000001000,
                player: player.clone(),
                bag_type: LootBagType::White,
                mob_type: 0,
                mob_name: "Unknown".to_string(),
                items: vec![NewLootItem {
                    slot: 0,
                    item_id: 14866,
                    item_name: "Saif of the Deep".to_string(),
                    enchant_ids: vec![],
                }],
            })
            .unwrap();

        // Public/player bag with no dragon signal stays Unknown.
        let public = db
            .insert_loot_drop(&NewLootDrop {
                timestamp: 1700000002000,
                player: player.clone(),
                bag_type: LootBagType::Brown,
                mob_type: 0,
                mob_name: "Unknown".to_string(),
                items: vec![NewLootItem {
                    slot: 0,
                    item_id: 2809,
                    item_name: "Hydra Skin Armor".to_string(),
                    enchant_ids: vec![],
                }],
            })
            .unwrap();

        db.migrate(4).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        let pyyr_row = drops.iter().find(|d| d.id == pyyr).unwrap();
        let nikao_row = drops.iter().find(|d| d.id == nikao).unwrap();
        let public_row = drops.iter().find(|d| d.id == public).unwrap();

        assert_eq!(pyyr_row.mob_type, super::super::LOD_PYYR_OBJECT_TYPE);
        assert_eq!(pyyr_row.mob_name, "Pyyr the Crimson Dragon");
        assert_eq!(nikao_row.mob_type, super::super::LOD_NIKAO_OBJECT_TYPE);
        assert_eq!(nikao_row.mob_name, "Nikao the Azure Dragon");
        assert_eq!(public_row.mob_type, 0);
        assert_eq!(public_row.mob_name, "Unknown");
    }

    #[test]
    fn migration_v6_backfills_spectral_penitentiary_bosses() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        let mut player = create_test_player();
        player.dungeon = super::super::SPECTRAL_PENITENTIARY_NAME.to_string();

        let mut insert = |ts: i64, bag: LootBagType, items: Vec<(i32, &str)>| {
            db.insert_loot_drop(&NewLootDrop {
                timestamp: ts,
                player: player.clone(),
                bag_type: bag,
                mob_type: 0,
                mob_name: "Unknown".to_string(),
                items: items
                    .into_iter()
                    .enumerate()
                    .map(|(slot, (item_id, item_name))| NewLootItem {
                        slot: slot as i32,
                        item_id,
                        item_name: item_name.to_string(),
                        enchant_ids: vec![],
                    })
                    .collect(),
            })
            .unwrap()
        };

        // Mana-paired Wisdom potion identifies Oculon.
        let oculon = insert(
            1700000000000,
            LootBagType::Blue,
            vec![(9068, "Greater Potion of Wisdom"), (2794, "Potion of Mana")],
        );

        // Mark of the Soulwarden identifies Murcian (does not count as a miniboss).
        let murcian = insert(
            1700000001000,
            LootBagType::White,
            vec![(
                super::super::MARK_OF_THE_SOULWARDEN_ITEM_ID,
                "Mark of the Soulwarden",
            )],
        );

        // A dungeon white identifies its miniboss without a potion.
        let zole = insert(
            1700000001500,
            LootBagType::White,
            vec![(23915, "Zole White")],
        );

        // Brown bag with a boss signal is a public bag and stays Unknown.
        let brown = insert(
            1700000002000,
            LootBagType::Brown,
            vec![(9068, "Greater Potion of Wisdom"), (2794, "Potion of Mana")],
        );

        // A lone stat potion (no mana) is a clearing-phase minion drop.
        let lone = insert(
            1700000003000,
            LootBagType::Blue,
            vec![(9068, "Greater Potion of Wisdom")],
        );

        // No boss signal stays Unknown.
        let plain = insert(
            1700000004000,
            LootBagType::Blue,
            vec![(2794, "Potion of Mana")],
        );

        db.migrate(5).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        let f = |id: i64| drops.iter().find(|d| d.id == id).unwrap();

        assert_eq!(f(oculon).mob_type, super::super::SPEN_OCULON_OBJECT_TYPE);
        assert_eq!(f(oculon).mob_name, "Overseer Oculon");
        assert_eq!(f(murcian).mob_type, super::super::SPEN_MURCIAN_OBJECT_TYPE);
        assert_eq!(f(murcian).mob_name, "Soulwarden Murcian");
        assert_eq!(f(zole).mob_type, super::super::SPEN_ZOLE_OBJECT_TYPE);
        assert_eq!(f(brown).mob_type, 0);
        assert_eq!(f(lone).mob_type, 0);
        assert_eq!(f(plain).mob_type, 0);
    }

    #[test]
    fn migration_v6_rejects_third_miniboss_by_dungeon_rule() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        let mut player = create_test_player();
        player.dungeon = super::super::SPECTRAL_PENITENTIARY_NAME.to_string();

        let mut insert = |ts: i64, item_id: i32| {
            db.insert_loot_drop(&NewLootDrop {
                timestamp: ts,
                player: player.clone(),
                bag_type: LootBagType::White,
                mob_type: 0,
                mob_name: "Unknown".to_string(),
                items: vec![NewLootItem {
                    slot: 0,
                    item_id,
                    item_name: "item".to_string(),
                    enchant_ids: vec![],
                }],
            })
            .unwrap()
        };

        // Two whites confirm Zole and Gretch: the run's two miniboss slots.
        let zole = insert(1700000000000, 23915);
        let gretch = insert(1700000001000, 7295);
        // A mana-paired Wisdom potion would be a third miniboss (Oculon): rejected.
        let oculon = db
            .insert_loot_drop(&NewLootDrop {
                timestamp: 1700000002000,
                player: player.clone(),
                bag_type: LootBagType::Blue,
                mob_type: 0,
                mob_name: "Unknown".to_string(),
                items: vec![
                    NewLootItem {
                        slot: 0,
                        item_id: 9068,
                        item_name: "Wisdom".to_string(),
                        enchant_ids: vec![],
                    },
                    NewLootItem {
                        slot: 1,
                        item_id: 2794,
                        item_name: "Mana".to_string(),
                        enchant_ids: vec![],
                    },
                ],
            })
            .unwrap();

        db.migrate(5).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        let f = |id: i64| drops.iter().find(|d| d.id == id).unwrap();
        assert_eq!(f(zole).mob_type, super::super::SPEN_ZOLE_OBJECT_TYPE);
        assert_eq!(f(gretch).mob_type, super::super::SPEN_GRETCH_OBJECT_TYPE);
        assert_eq!(f(oculon).mob_type, 0);
    }

    #[test]
    fn migration_v6_backfills_spectral_penitentiary_refined_signals() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        let mut player = create_test_player();
        player.dungeon = super::super::SPECTRAL_PENITENTIARY_NAME.to_string();

        // Each bag is placed in its own run (distinct map_seed) so the Oculon
        // default is tested in isolation, without a confirmed miniboss absorbing
        // the weaker signals.
        let mut insert = |seed: i32, bag: LootBagType, items: Vec<(i32, &str)>| {
            let mut p = player.clone();
            p.map_seed = seed;
            db.insert_loot_drop(&NewLootDrop {
                timestamp: 1700000000000,
                player: p,
                bag_type: bag,
                mob_type: 0,
                mob_name: "Unknown".to_string(),
                items: items
                    .into_iter()
                    .enumerate()
                    .map(|(slot, (item_id, item_name))| NewLootItem {
                        slot: slot as i32,
                        item_id,
                        item_name: item_name.to_string(),
                        enchant_ids: vec![],
                    })
                    .collect(),
            })
            .unwrap()
        };

        // A forge blueprint reliably identifies its miniboss (Gretch here).
        let blueprint = insert(1, LootBagType::White, vec![(44408, "Blueprint")]);

        // A Schematic proves a miniboss drop but no identity: defaults to Oculon.
        let schematic = insert(2, LootBagType::Blue, vec![(34634, "Schematic")]);

        // An Alchemist Assassin shard narrows to the Oculon/Gretch pair; with no
        // other signal it defaults to Oculon.
        let alch_shard = insert(3, LootBagType::Blue, vec![(53361, "Shard")]);

        // A Helmet Rune is boss-only and narrows to Oculon/Gretch: Oculon default.
        let rune = insert(4, LootBagType::Blue, vec![(10024, "Helmet Rune")]);

        db.migrate(5).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        let f = |id: i64| drops.iter().find(|d| d.id == id).unwrap();
        assert_eq!(f(blueprint).mob_type, super::super::SPEN_GRETCH_OBJECT_TYPE);
        assert_eq!(f(schematic).mob_type, super::super::SPEN_OCULON_OBJECT_TYPE);
        assert_eq!(
            f(alch_shard).mob_type,
            super::super::SPEN_OCULON_OBJECT_TYPE
        );
        assert_eq!(f(rune).mob_type, super::super::SPEN_OCULON_OBJECT_TYPE);
    }

    #[test]
    fn migration_v6_plague_shard_without_confirm_stays_unknown() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        let mut player = create_test_player();
        player.dungeon = super::super::SPECTRAL_PENITENTIARY_NAME.to_string();

        let plague = db
            .insert_loot_drop(&NewLootDrop {
                timestamp: 1700000000000,
                player: player.clone(),
                bag_type: LootBagType::Blue,
                mob_type: 0,
                mob_name: "Unknown".to_string(),
                items: vec![NewLootItem {
                    slot: 0,
                    item_id: 53297,
                    item_name: "Plague Doctor Priest Shard".to_string(),
                    enchant_ids: vec![],
                }],
            })
            .unwrap();

        db.migrate(5).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        // Plague pair excludes Oculon and neither Lobotomik nor Zole is confirmed
        // in the run, so the bag stays Unknown.
        assert_eq!(drops.iter().find(|d| d.id == plague).unwrap().mob_type, 0);
    }

    /// Insert a Tomb bag in a given run (`seed`) with a starting attribution.
    /// Item ids here are deliberately outside every Tomb drop table so the
    /// asset-independent run-grouping rules (alphabetical fallback, seed guard,
    /// brown skip, attributed seeding) are what gets exercised.
    fn insert_tomb_bag(
        db: &mut LootDatabase,
        seed: i32,
        bag: LootBagType,
        mob_type: i32,
        mob_name: &str,
        items: Vec<(i32, &str)>,
    ) -> i64 {
        let mut player = create_test_player();
        player.dungeon = super::super::TOMB_OF_THE_ANCIENTS_NAME.to_string();
        player.map_seed = seed;
        db.insert_loot_drop(&NewLootDrop {
            timestamp: 1700000000000,
            player,
            bag_type: bag,
            mob_type,
            mob_name: mob_name.to_string(),
            items: items
                .into_iter()
                .enumerate()
                .map(|(slot, (item_id, item_name))| NewLootItem {
                    slot: slot as i32,
                    item_id,
                    item_name: item_name.to_string(),
                    enchant_ids: vec![],
                })
                .collect(),
        })
        .unwrap()
    }

    #[test]
    fn migration_v8_assigns_unknown_tomb_bags_alphabetically_per_run() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        // Two Unknown offscreen bags in one run (no unique loot): they go to the
        // alphabetically-first un-credited bosses, one per boss.
        let first = insert_tomb_bag(
            &mut db,
            4242,
            LootBagType::White,
            0,
            "Unknown",
            vec![(9999, "Junk")],
        );
        let second = insert_tomb_bag(
            &mut db,
            4242,
            LootBagType::White,
            0,
            "Unknown",
            vec![(9998, "Junk2")],
        );

        db.migrate(7).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        let f = |id: i64| drops.iter().find(|d| d.id == id).unwrap();
        assert_eq!(f(first).mob_type, super::super::TOMB_BES_OBJECT_TYPE);
        assert_eq!(f(second).mob_type, super::super::TOMB_GEB_OBJECT_TYPE);
    }

    #[test]
    fn migration_v8_leaves_seed_zero_unknown() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        // Seed 0 means the run is unknown; distinct runs could be merged, so the
        // alphabetical guess must not fire.
        let bag = insert_tomb_bag(
            &mut db,
            0,
            LootBagType::White,
            0,
            "Unknown",
            vec![(9999, "Junk")],
        );

        db.migrate(7).unwrap();

        assert_eq!(
            db.get_recent_drops(10)
                .unwrap()
                .iter()
                .find(|d| d.id == bag)
                .unwrap()
                .mob_type,
            0
        );
    }

    #[test]
    fn migration_v8_skips_brown_bags() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        // Brown/public bags are never boss drops.
        let bag = insert_tomb_bag(
            &mut db,
            4242,
            LootBagType::Brown,
            0,
            "Unknown",
            vec![(9999, "Junk")],
        );

        db.migrate(7).unwrap();

        assert_eq!(
            db.get_recent_drops(10)
                .unwrap()
                .iter()
                .find(|d| d.id == bag)
                .unwrap()
                .mob_type,
            0
        );
    }

    #[test]
    fn migration_v8_alphabetical_skips_already_credited_boss() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        // First bag is already (correctly) attributed to Bes by proximity; the
        // later Unknown bag must skip Bes and take the next boss, Geb.
        let bes = insert_tomb_bag(
            &mut db,
            4242,
            LootBagType::White,
            super::super::TOMB_BES_OBJECT_TYPE,
            "Bes",
            vec![(9999, "Junk")],
        );
        let unknown = insert_tomb_bag(
            &mut db,
            4242,
            LootBagType::White,
            0,
            "Unknown",
            vec![(9998, "Junk2")],
        );

        db.migrate(7).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        let f = |id: i64| drops.iter().find(|d| d.id == id).unwrap();
        // Bes bag is untouched; the Unknown bag becomes Geb.
        assert_eq!(f(bes).mob_type, super::super::TOMB_BES_OBJECT_TYPE);
        assert_eq!(f(unknown).mob_type, super::super::TOMB_GEB_OBJECT_TYPE);
    }

    #[test]
    fn migration_v8_runs_are_independent() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        // Each run credits its own bosses; the first Unknown bag of every run
        // gets Bes.
        let run_a = insert_tomb_bag(
            &mut db,
            100,
            LootBagType::White,
            0,
            "Unknown",
            vec![(9999, "Junk")],
        );
        let run_b = insert_tomb_bag(
            &mut db,
            200,
            LootBagType::White,
            0,
            "Unknown",
            vec![(9998, "Junk2")],
        );

        db.migrate(7).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        let f = |id: i64| drops.iter().find(|d| d.id == id).unwrap();
        assert_eq!(f(run_a).mob_type, super::super::TOMB_BES_OBJECT_TYPE);
        assert_eq!(f(run_b).mob_type, super::super::TOMB_BES_OBJECT_TYPE);
    }

    #[test]
    fn migration_v8_fourth_unknown_bag_stays_unknown() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        // Only three main bosses, so a run's fourth Unknown bag has no boss left.
        let ids: Vec<i64> = (0..4)
            .map(|i| {
                insert_tomb_bag(
                    &mut db,
                    777,
                    LootBagType::White,
                    0,
                    "Unknown",
                    vec![(9000 + i, "Junk")],
                )
            })
            .collect();

        db.migrate(7).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        let f = |id: i64| drops.iter().find(|d| d.id == id).unwrap();
        assert_eq!(f(ids[0]).mob_type, super::super::TOMB_BES_OBJECT_TYPE);
        assert_eq!(f(ids[1]).mob_type, super::super::TOMB_GEB_OBJECT_TYPE);
        assert_eq!(f(ids[2]).mob_type, super::super::TOMB_NUT_OBJECT_TYPE);
        assert_eq!(f(ids[3]).mob_type, 0);
    }

    #[test]
    fn migration_v10_corrects_mark_bag_and_frees_geb_for_life() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        // The user's real bug: the Mark of Geb bag was wrongly attributed to Nut
        // (proximity) and seeded Nut, while an Unknown Life bag that should be
        // Geb stayed Unknown. With item-id detection the Mark bag is forced to
        // Geb (without consuming Geb's slot) and the Life bag becomes Geb.
        let bes = insert_tomb_bag(
            &mut db,
            1858913612,
            LootBagType::Blue,
            super::super::TOMB_BES_OBJECT_TYPE,
            "Bes",
            vec![(super::super::POTION_OF_LIFE_ITEM_ID, "Potion of Life")],
        );
        let nut = insert_tomb_bag(
            &mut db,
            1858913612,
            LootBagType::Blue,
            super::super::TOMB_NUT_OBJECT_TYPE,
            "Nut",
            vec![(super::super::POTION_OF_LIFE_ITEM_ID, "Potion of Life")],
        );
        // Mark bag mis-attributed to Nut before the fix.
        let mark = insert_tomb_bag(
            &mut db,
            1858913612,
            LootBagType::White,
            super::super::TOMB_NUT_OBJECT_TYPE,
            "Nut",
            vec![(super::super::MARK_OF_GEB_ITEM_ID, "Mark of Geb")],
        );
        let orphan = insert_tomb_bag(
            &mut db,
            1858913612,
            LootBagType::Blue,
            0,
            "Unknown",
            vec![(super::super::POTION_OF_LIFE_ITEM_ID, "Potion of Life")],
        );

        db.migrate(7).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        let f = |id: i64| drops.iter().find(|d| d.id == id).unwrap();
        assert_eq!(f(bes).mob_type, super::super::TOMB_BES_OBJECT_TYPE);
        assert_eq!(f(nut).mob_type, super::super::TOMB_NUT_OBJECT_TYPE);
        assert_eq!(f(mark).mob_type, super::super::TOMB_GEB_OBJECT_TYPE);
        assert_eq!(f(orphan).mob_type, super::super::TOMB_GEB_OBJECT_TYPE);
    }

    #[test]
    fn migration_v9_fills_geb_slot_left_by_mark_bag() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        // Simulate the v8 aftermath: two Life bags already correctly credited to
        // Bes and Nut, plus a still-Unknown bag that should be Geb. (The Mark of
        // Geb bag that wrongly consumed Geb's slot no longer seeds `attributed`.)
        let bes = insert_tomb_bag(
            &mut db,
            555,
            LootBagType::Blue,
            super::super::TOMB_BES_OBJECT_TYPE,
            "Bes",
            vec![(9001, "Junk")],
        );
        let nut = insert_tomb_bag(
            &mut db,
            555,
            LootBagType::Blue,
            super::super::TOMB_NUT_OBJECT_TYPE,
            "Nut",
            vec![(9002, "Junk")],
        );
        let orphan = insert_tomb_bag(
            &mut db,
            555,
            LootBagType::Blue,
            0,
            "Unknown",
            vec![(9003, "Junk")],
        );

        // Coming from v8 (only the v9 re-run applies).
        db.migrate(8).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        let f = |id: i64| drops.iter().find(|d| d.id == id).unwrap();
        assert_eq!(f(bes).mob_type, super::super::TOMB_BES_OBJECT_TYPE);
        assert_eq!(f(nut).mob_type, super::super::TOMB_NUT_OBJECT_TYPE);
        assert_eq!(f(orphan).mob_type, super::super::TOMB_GEB_OBJECT_TYPE);
    }

    #[test]
    fn migration_v8_is_idempotent_when_rerun() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        let a = insert_tomb_bag(
            &mut db,
            888,
            LootBagType::White,
            0,
            "Unknown",
            vec![(9101, "Junk")],
        );
        let b = insert_tomb_bag(
            &mut db,
            888,
            LootBagType::White,
            0,
            "Unknown",
            vec![(9102, "Junk")],
        );

        // Full chain (v7 backfill, v8 backfill, v9 re-run) must be stable.
        db.migrate(7).unwrap();
        let after_first: Vec<i32> = db
            .get_recent_drops(10)
            .unwrap()
            .iter()
            .map(|d| d.mob_type)
            .collect();

        // Re-running the backfill directly changes nothing further.
        db.backfill_tomb_of_the_ancients().unwrap();
        let after_second: Vec<i32> = db
            .get_recent_drops(10)
            .unwrap()
            .iter()
            .map(|d| d.mob_type)
            .collect();

        assert_eq!(after_first, after_second);
        let drops = db.get_recent_drops(10).unwrap();
        let f = |id: i64| drops.iter().find(|d| d.id == id).unwrap();
        assert_eq!(f(a).mob_type, super::super::TOMB_BES_OBJECT_TYPE);
        assert_eq!(f(b).mob_type, super::super::TOMB_GEB_OBJECT_TYPE);
    }

    #[test]
    fn migration_v11_redistributes_doubled_geb_life_to_nut() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        // The user's bug: proximity stacked two Life bags on Geb while Nut (which
        // the Mark of Geb proves also died) got none. The second Life bag must be
        // handed to the un-credited main boss (Nut).
        let bes = insert_tomb_bag(
            &mut db,
            528080545,
            LootBagType::Blue,
            super::super::TOMB_BES_OBJECT_TYPE,
            "Bes",
            vec![(super::super::POTION_OF_LIFE_ITEM_ID, "Potion of Life")],
        );
        let geb1 = insert_tomb_bag(
            &mut db,
            528080545,
            LootBagType::Blue,
            super::super::TOMB_GEB_OBJECT_TYPE,
            "Geb",
            vec![(super::super::POTION_OF_LIFE_ITEM_ID, "Potion of Life")],
        );
        // Second Life bag wrongly stacked on Geb.
        let geb2 = insert_tomb_bag(
            &mut db,
            528080545,
            LootBagType::Blue,
            super::super::TOMB_GEB_OBJECT_TYPE,
            "Geb",
            vec![(super::super::POTION_OF_LIFE_ITEM_ID, "Potion of Life")],
        );
        let mark = insert_tomb_bag(
            &mut db,
            528080545,
            LootBagType::White,
            super::super::TOMB_GEB_OBJECT_TYPE,
            "Geb",
            vec![(super::super::MARK_OF_GEB_ITEM_ID, "Mark of Geb")],
        );

        // Coming from v10 (only the v11 re-run applies).
        db.migrate(10).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        let f = |id: i64| drops.iter().find(|d| d.id == id).unwrap();
        assert_eq!(f(bes).mob_type, super::super::TOMB_BES_OBJECT_TYPE);
        assert_eq!(f(geb1).mob_type, super::super::TOMB_GEB_OBJECT_TYPE);
        // The doubled bag is redistributed to the un-credited boss, Nut.
        assert_eq!(f(geb2).mob_type, super::super::TOMB_NUT_OBJECT_TYPE);
        // Mark bag stays Geb without consuming a slot.
        assert_eq!(f(mark).mob_type, super::super::TOMB_GEB_OBJECT_TYPE);
    }

    #[test]
    fn migration_v11_redistributes_doubled_bes_to_geb() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        // Two Life bags on Bes, one on Nut, none on Geb (Mark proves Geb died).
        let bes1 = insert_tomb_bag(
            &mut db,
            835116835,
            LootBagType::Blue,
            super::super::TOMB_BES_OBJECT_TYPE,
            "Bes",
            vec![(super::super::POTION_OF_LIFE_ITEM_ID, "Potion of Life")],
        );
        let bes2 = insert_tomb_bag(
            &mut db,
            835116835,
            LootBagType::Blue,
            super::super::TOMB_BES_OBJECT_TYPE,
            "Bes",
            vec![(super::super::POTION_OF_LIFE_ITEM_ID, "Potion of Life")],
        );
        let nut = insert_tomb_bag(
            &mut db,
            835116835,
            LootBagType::Blue,
            super::super::TOMB_NUT_OBJECT_TYPE,
            "Nut",
            vec![(super::super::POTION_OF_LIFE_ITEM_ID, "Potion of Life")],
        );
        let mark = insert_tomb_bag(
            &mut db,
            835116835,
            LootBagType::White,
            super::super::TOMB_GEB_OBJECT_TYPE,
            "Geb",
            vec![(super::super::MARK_OF_GEB_ITEM_ID, "Mark of Geb")],
        );

        db.migrate(10).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        let f = |id: i64| drops.iter().find(|d| d.id == id).unwrap();
        assert_eq!(f(bes1).mob_type, super::super::TOMB_BES_OBJECT_TYPE);
        // Second Bes bag redistributed to the only free main boss, Geb.
        assert_eq!(f(bes2).mob_type, super::super::TOMB_GEB_OBJECT_TYPE);
        assert_eq!(f(nut).mob_type, super::super::TOMB_NUT_OBJECT_TYPE);
        assert_eq!(f(mark).mob_type, super::super::TOMB_GEB_OBJECT_TYPE);
    }

    #[test]
    fn migration_v11_leaves_sarcophagus_life_untouched() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        // The Sarcophagus miniboss is a legitimate separate Life dropper; its bag
        // must not be redistributed to a main boss nor consume a main boss slot.
        let sarc = insert_tomb_bag(
            &mut db,
            4321,
            LootBagType::Blue,
            super::super::TOMB_ACTIVE_SARCOPHAGUS_OBJECT_TYPE,
            "Active Sarcophagus",
            vec![(super::super::POTION_OF_LIFE_ITEM_ID, "Potion of Life")],
        );
        let bes = insert_tomb_bag(
            &mut db,
            4321,
            LootBagType::Blue,
            super::super::TOMB_BES_OBJECT_TYPE,
            "Bes",
            vec![(super::super::POTION_OF_LIFE_ITEM_ID, "Potion of Life")],
        );
        // An Unknown Life bag should still get the alphabetically-first free boss
        // (Geb), proving the Sarcophagus did not consume Bes's or anyone's slot.
        let orphan = insert_tomb_bag(
            &mut db,
            4321,
            LootBagType::Blue,
            0,
            "Unknown",
            vec![(super::super::POTION_OF_LIFE_ITEM_ID, "Potion of Life")],
        );

        db.migrate(10).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        let f = |id: i64| drops.iter().find(|d| d.id == id).unwrap();
        assert_eq!(
            f(sarc).mob_type,
            super::super::TOMB_ACTIVE_SARCOPHAGUS_OBJECT_TYPE
        );
        assert_eq!(f(bes).mob_type, super::super::TOMB_BES_OBJECT_TYPE);
        assert_eq!(f(orphan).mob_type, super::super::TOMB_GEB_OBJECT_TYPE);
    }

    #[test]
    fn migration_v14_orphan_life_bag_before_boss_bags_goes_to_geb() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        // Reported v0.18 run: Geb died offscreen and his Life bag was discovered
        // first (lowest id) and stored Unknown, while Bes and Nut were credited
        // on-screen; the Mark of Geb confirms all three died. The orphan must go
        // to Geb WITHOUT bumping the correctly-stored Bes/Nut bags -- preferred
        // main-boss bags are reserved before orphan bags are distributed, even
        // when an orphan has a lower id.
        let orphan = insert_tomb_bag(
            &mut db,
            771122,
            LootBagType::Blue,
            0,
            "Unknown",
            vec![(super::super::POTION_OF_LIFE_ITEM_ID, "Potion of Life")],
        );
        let bes = insert_tomb_bag(
            &mut db,
            771122,
            LootBagType::Blue,
            super::super::TOMB_BES_OBJECT_TYPE,
            "Bes",
            vec![(super::super::POTION_OF_LIFE_ITEM_ID, "Potion of Life")],
        );
        let nut = insert_tomb_bag(
            &mut db,
            771122,
            LootBagType::Blue,
            super::super::TOMB_NUT_OBJECT_TYPE,
            "Nut",
            vec![(super::super::POTION_OF_LIFE_ITEM_ID, "Potion of Life")],
        );
        let mark = insert_tomb_bag(
            &mut db,
            771122,
            LootBagType::White,
            super::super::TOMB_GEB_OBJECT_TYPE,
            "Geb",
            vec![(super::super::MARK_OF_GEB_ITEM_ID, "Mark of Geb")],
        );

        // Coming from v13: only the v14 re-run applies.
        db.migrate(13).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        let f = |id: i64| drops.iter().find(|d| d.id == id).unwrap();
        assert_eq!(f(bes).mob_type, super::super::TOMB_BES_OBJECT_TYPE);
        assert_eq!(f(nut).mob_type, super::super::TOMB_NUT_OBJECT_TYPE);
        assert_eq!(f(orphan).mob_type, super::super::TOMB_GEB_OBJECT_TYPE);
        assert_eq!(f(mark).mob_type, super::super::TOMB_GEB_OBJECT_TYPE);
    }

    #[test]
    fn migration_v12_spawner_bag_frees_stolen_boss_slot() {
        // Needs real assets to categorize the cookie as a spawner item.
        let mgr = crate::assets::get_asset_manager();
        if let Some(dir) = crate::assets::find_assets_dir() {
            mgr.set_assets_dir(&dir);
        }
        // 3268 = Chocolate Cream Sandwich Cookie (pet food).
        if !mgr.try_load() || mgr.get_object(3268).is_none() {
            eprintln!("skipping: game assets not available");
            return;
        }

        let mut db = LootDatabase::open_in_memory().unwrap();
        // The user's bug (run 1540380110): proximity pinned a cookie-only spawner
        // bag to Nut, stealing her slot, so Nut's real Life bag became Unknown.
        let bes = insert_tomb_bag(
            &mut db,
            1540380110,
            LootBagType::Blue,
            super::super::TOMB_BES_OBJECT_TYPE,
            "Bes",
            vec![(super::super::POTION_OF_LIFE_ITEM_ID, "Potion of Life")],
        );
        let geb = insert_tomb_bag(
            &mut db,
            1540380110,
            LootBagType::Blue,
            super::super::TOMB_GEB_OBJECT_TYPE,
            "Geb",
            vec![(super::super::POTION_OF_LIFE_ITEM_ID, "Potion of Life")],
        );
        let cookie = insert_tomb_bag(
            &mut db,
            1540380110,
            LootBagType::White,
            super::super::TOMB_NUT_OBJECT_TYPE,
            "Nut",
            vec![(3268, "Chocolate Cream Sandwich Cookie")],
        );
        let orphan = insert_tomb_bag(
            &mut db,
            1540380110,
            LootBagType::Blue,
            0,
            "Unknown",
            vec![(super::super::POTION_OF_LIFE_ITEM_ID, "Potion of Life")],
        );

        db.migrate(11).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        let f = |id: i64| drops.iter().find(|d| d.id == id).unwrap();
        assert_eq!(f(bes).mob_type, super::super::TOMB_BES_OBJECT_TYPE);
        assert_eq!(f(geb).mob_type, super::super::TOMB_GEB_OBJECT_TYPE);
        // Cookie bag credited to Geb (no slot); Nut's real bag recovered.
        assert_eq!(f(cookie).mob_type, super::super::TOMB_GEB_OBJECT_TYPE);
        assert_eq!(f(orphan).mob_type, super::super::TOMB_NUT_OBJECT_TYPE);
    }

    #[test]
    fn find_drops_for_run_matches_instance_and_window() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        let mut player = create_test_player();
        player.dungeon = "The Void".to_string();
        player.map_seed = 5;

        let base = 1_700_000_000_000i64;
        for (ts, mob) in [(base, 45076), (base + 1000, 45076), (base + 2000, 99999)] {
            db.insert_loot_drop(&NewLootDrop {
                timestamp: ts,
                player: player.clone(),
                bag_type: LootBagType::White,
                mob_type: mob,
                mob_name: "Void Entity".to_string(),
                items: vec![NewLootItem {
                    slot: 0,
                    item_id: 9000,
                    item_name: "Void Blade".to_string(),
                    enchant_ids: vec![],
                }],
            })
            .unwrap();
        }

        // Two drops from mob 45076 within the window, ordered oldest-first, with items.
        let hits = db
            .find_drops_for_run(5, &[45076], base - 1000, base + 5000)
            .unwrap();
        assert_eq!(hits.len(), 2);
        assert!(hits[0].timestamp <= hits[1].timestamp);
        assert_eq!(hits[0].items.len(), 1);

        // Multiple boss types union in the drop from mob 99999 too.
        let both = db
            .find_drops_for_run(5, &[45076, 99999], base - 1000, base + 5000)
            .unwrap();
        assert_eq!(both.len(), 3);

        // Wrong seed, empty types, zero seed, and a tight window all miss.
        assert!(db
            .find_drops_for_run(9, &[45076], base - 1000, base + 5000)
            .unwrap()
            .is_empty());
        assert!(db
            .find_drops_for_run(5, &[], base - 1000, base + 5000)
            .unwrap()
            .is_empty());
        assert!(db
            .find_drops_for_run(0, &[45076], base - 1000, base + 5000)
            .unwrap()
            .is_empty());
        assert!(db
            .find_drops_for_run(5, &[45076], base + 3000, base + 5000)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn test_query_by_dungeon() {
        let mut db = LootDatabase::open_in_memory().unwrap();

        // Insert drop in The Void
        let mut player = create_test_player();
        player.dungeon = "The Void".to_string();
        let drop1 = NewLootDrop {
            timestamp: 1700000000000,
            player,
            bag_type: LootBagType::White,
            mob_type: 45076,
            mob_name: "Void Entity".to_string(),
            items: vec![],
        };
        db.insert_loot_drop(&drop1).unwrap();

        // Insert drop in Oryx's Castle
        let mut player2 = create_test_player();
        player2.dungeon = "Oryx's Castle".to_string();
        let drop2 = NewLootDrop {
            timestamp: 1700000001000,
            player: player2,
            bag_type: LootBagType::Orange,
            mob_type: 1234,
            mob_name: "Oryx".to_string(),
            items: vec![],
        };
        db.insert_loot_drop(&drop2).unwrap();

        // Query by dungeon
        let void_drops = db.get_drops_by_dungeon("The Void", 100).unwrap();
        assert_eq!(void_drops.len(), 1);

        let castle_drops = db.get_drops_by_dungeon("Oryx's Castle", 100).unwrap();
        assert_eq!(castle_drops.len(), 1);
    }

    #[test]
    fn test_query_by_bag_type() {
        let mut db = LootDatabase::open_in_memory().unwrap();

        // Insert white bag
        let drop1 = NewLootDrop {
            timestamp: 1700000000000,
            player: create_test_player(),
            bag_type: LootBagType::White,
            mob_type: 1,
            mob_name: "Boss1".to_string(),
            items: vec![],
        };
        db.insert_loot_drop(&drop1).unwrap();

        // Insert orange bag
        let drop2 = NewLootDrop {
            timestamp: 1700000001000,
            player: create_test_player(),
            bag_type: LootBagType::Orange,
            mob_type: 2,
            mob_name: "Boss2".to_string(),
            items: vec![],
        };
        db.insert_loot_drop(&drop2).unwrap();

        let white_drops = db.get_drops_by_bag_type(LootBagType::White, 100).unwrap();
        assert_eq!(white_drops.len(), 1);

        let orange_drops = db.get_drops_by_bag_type(LootBagType::Orange, 100).unwrap();
        assert_eq!(orange_drops.len(), 1);
    }

    #[test]
    fn test_statistics() {
        let mut db = LootDatabase::open_in_memory().unwrap();

        // Insert 2 white bags, 1 orange
        for i in 0..2 {
            let drop = NewLootDrop {
                timestamp: 1700000000000 + i * 1000,
                player: create_test_player(),
                bag_type: LootBagType::White,
                mob_type: 1,
                mob_name: "Boss".to_string(),
                items: vec![],
            };
            db.insert_loot_drop(&drop).unwrap();
        }

        let drop = NewLootDrop {
            timestamp: 1700000003000,
            player: create_test_player(),
            bag_type: LootBagType::Orange,
            mob_type: 2,
            mob_name: "Boss2".to_string(),
            items: vec![],
        };
        db.insert_loot_drop(&drop).unwrap();

        let stats = db.get_statistics().unwrap();
        assert_eq!(stats.total_drops, 3);
        assert_eq!(stats.white_bags, 2);
        assert_eq!(stats.orange_bags, 1);
    }

    #[test]
    fn test_enchant_parsing() {
        let item = LootItemRecord {
            id: 1,
            drop_id: 1,
            slot: 0,
            item_id: 9000,
            item_name: "Test".to_string(),
            enchant_count: 3,
            enchant_ids: "100,200,300".to_string(),
            parsed_enchant_ids: vec![100, 200, 300],
        };

        let enchants = item.enchant_id_list();
        assert_eq!(enchants, &[100, 200, 300]);
    }

    #[test]
    fn test_clear_all() {
        let mut db = LootDatabase::open_in_memory().unwrap();

        let drop = NewLootDrop {
            timestamp: 1700000000000,
            player: create_test_player(),
            bag_type: LootBagType::White,
            mob_type: 1,
            mob_name: "Boss".to_string(),
            items: vec![],
        };
        db.insert_loot_drop(&drop).unwrap();

        assert_eq!(db.total_drops().unwrap(), 1);

        db.clear_all().unwrap();

        assert_eq!(db.total_drops().unwrap(), 0);
    }

    #[test]
    fn test_delete_drops_removes_rows_and_items() {
        let mut db = LootDatabase::open_in_memory().unwrap();

        let mut ids = Vec::new();
        for i in 0..3 {
            let drop = NewLootDrop {
                timestamp: 1700000000000 + i,
                player: create_test_player(),
                bag_type: LootBagType::White,
                mob_type: 1,
                mob_name: "Boss".to_string(),
                items: vec![NewLootItem {
                    slot: 0,
                    item_id: 9000,
                    item_name: "Void Blade".to_string(),
                    enchant_ids: vec![],
                }],
            };
            ids.push(db.insert_loot_drop(&drop).unwrap());
        }
        assert_eq!(db.total_drops().unwrap(), 3);

        // Delete two of the three drops.
        let deleted = db.delete_drops(&[ids[0], ids[2]]).unwrap();
        assert_eq!(deleted, 2);
        assert_eq!(db.total_drops().unwrap(), 1);

        // The surviving drop keeps its item; the deleted drops' items are gone.
        let remaining = db.get_recent_drops(10).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, ids[1]);
        assert_eq!(remaining[0].items.len(), 1);
        let orphan_items: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM loot_items WHERE drop_id IN (?1, ?2)",
                params![ids[0], ids[2]],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphan_items, 0, "deleted drops' items must be removed");

        // Empty input is a no-op.
        assert_eq!(db.delete_drops(&[]).unwrap(), 0);
    }

    #[test]
    fn test_source_filters_is_empty() {
        let mut filters = SourceFilters::default();
        assert!(filters.is_empty());

        filters.set_mob(123, "Test Mob".to_string());
        assert!(!filters.is_empty());

        filters.clear_mob();
        assert!(filters.is_empty());

        filters.set_dungeon("Test Dungeon".to_string());
        assert!(!filters.is_empty());

        filters.clear();
        assert!(filters.is_empty());
    }

    #[test]
    fn test_filtered_query_no_filters() {
        let mut db = LootDatabase::open_in_memory().unwrap();

        // Insert two drops
        let drop1 = NewLootDrop {
            timestamp: 1700000000000,
            player: create_test_player(),
            bag_type: LootBagType::White,
            mob_type: 100,
            mob_name: "Mob A".to_string(),
            items: vec![],
        };
        db.insert_loot_drop(&drop1).unwrap();

        let drop2 = NewLootDrop {
            timestamp: 1700000001000,
            player: create_test_player(),
            bag_type: LootBagType::Orange,
            mob_type: 200,
            mob_name: "Mob B".to_string(),
            items: vec![],
        };
        db.insert_loot_drop(&drop2).unwrap();

        // Query with no filters should return both
        let filters = SourceFilters::default();
        let drops = db
            .get_drops_filtered(0, i64::MAX, &filters, 0, 100)
            .unwrap();
        assert_eq!(drops.len(), 2);
    }

    #[test]
    fn test_filtered_query_empty_bag_types_matches_nothing() {
        let mut db = LootDatabase::open_in_memory().unwrap();

        let drop1 = NewLootDrop {
            timestamp: 1700000000000,
            player: create_test_player(),
            bag_type: LootBagType::White,
            mob_type: 100,
            mob_name: "Mob A".to_string(),
            items: vec![],
        };
        db.insert_loot_drop(&drop1).unwrap();

        // Some(empty) means the user disabled every bag type => match nothing.
        let mut filters = SourceFilters::default();
        filters.set_bag_types(Vec::new());
        let drops = db
            .get_drops_filtered(0, i64::MAX, &filters, 0, 100)
            .unwrap();
        assert_eq!(drops.len(), 0);
        let count = db.count_drops_filtered(0, i64::MAX, &filters).unwrap();
        assert_eq!(count, 0);

        // None means no bag-type restriction => match everything.
        let filters = SourceFilters::default();
        let drops = db
            .get_drops_filtered(0, i64::MAX, &filters, 0, 100)
            .unwrap();
        assert_eq!(drops.len(), 1);
    }

    #[test]
    fn test_filtered_query_mob_filter() {
        let mut db = LootDatabase::open_in_memory().unwrap();

        // Insert drops from different mobs
        let drop1 = NewLootDrop {
            timestamp: 1700000000000,
            player: create_test_player(),
            bag_type: LootBagType::White,
            mob_type: 100,
            mob_name: "Oryx 3".to_string(),
            items: vec![],
        };
        db.insert_loot_drop(&drop1).unwrap();

        let drop2 = NewLootDrop {
            timestamp: 1700000001000,
            player: create_test_player(),
            bag_type: LootBagType::White,
            mob_type: 200,
            mob_name: "Void Entity".to_string(),
            items: vec![],
        };
        db.insert_loot_drop(&drop2).unwrap();

        // Filter by mob_type 100
        let mut filters = SourceFilters::default();
        filters.set_mob(100, "Oryx 3".to_string());
        let drops = db
            .get_drops_filtered(0, i64::MAX, &filters, 0, 100)
            .unwrap();
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].mob_type, 100);
    }

    #[test]
    fn test_filtered_query_dungeon_filter() {
        let mut db = LootDatabase::open_in_memory().unwrap();

        // Insert drops from different dungeons
        let mut player1 = create_test_player();
        player1.dungeon = "Sanctuary".to_string();
        let drop1 = NewLootDrop {
            timestamp: 1700000000000,
            player: player1,
            bag_type: LootBagType::White,
            mob_type: 100,
            mob_name: "Oryx 3".to_string(),
            items: vec![],
        };
        db.insert_loot_drop(&drop1).unwrap();

        let mut player2 = create_test_player();
        player2.dungeon = "The Void".to_string();
        let drop2 = NewLootDrop {
            timestamp: 1700000001000,
            player: player2,
            bag_type: LootBagType::White,
            mob_type: 200,
            mob_name: "Void Entity".to_string(),
            items: vec![],
        };
        db.insert_loot_drop(&drop2).unwrap();

        // Filter by dungeon
        let mut filters = SourceFilters::default();
        filters.set_dungeon("Sanctuary".to_string());
        let drops = db
            .get_drops_filtered(0, i64::MAX, &filters, 0, 100)
            .unwrap();
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].dungeon, "Sanctuary");
    }

    #[test]
    fn test_filtered_query_search_text() {
        let mut db = LootDatabase::open_in_memory().unwrap();

        let mut p1 = create_test_player();
        p1.dungeon = "Sanctuary".to_string();
        db.insert_loot_drop(&NewLootDrop {
            timestamp: 1700000000000,
            player: p1,
            bag_type: LootBagType::White,
            mob_type: 100,
            mob_name: "Oryx 3".to_string(),
            items: vec![NewLootItem {
                slot: 0,
                item_id: 9000,
                item_name: "Wand of Ages".to_string(),
                enchant_ids: vec![],
            }],
        })
        .unwrap();

        let mut p2 = create_test_player();
        p2.dungeon = "The Void".to_string();
        db.insert_loot_drop(&NewLootDrop {
            timestamp: 1700000001000,
            player: p2,
            bag_type: LootBagType::White,
            mob_type: 200,
            mob_name: "Void Entity".to_string(),
            items: vec![NewLootItem {
                slot: 0,
                item_id: 9001,
                item_name: "Staff of Destruction".to_string(),
                enchant_ids: vec![],
            }],
        })
        .unwrap();

        let search = |text: &str| {
            let filters = SourceFilters {
                search_text: Some(text.to_string()),
                ..Default::default()
            };
            let drops = db
                .get_drops_filtered(0, i64::MAX, &filters, 0, 100)
                .unwrap();
            let count = db.count_drops_filtered(0, i64::MAX, &filters).unwrap();
            assert_eq!(drops.len() as i64, count);
            drops
        };

        // Match by mob name (case-insensitive).
        let d = search("oryx");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].mob_name, "Oryx 3");

        // Match by dungeon name.
        let d = search("void");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].dungeon, "The Void");

        // Match by item name.
        let d = search("wand");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].mob_name, "Oryx 3");

        // Empty/whitespace text matches everything.
        assert_eq!(search("   ").len(), 2);

        // No match.
        assert_eq!(search("nonexistent").len(), 0);
    }

    #[test]
    fn test_filtered_query_char_filter() {
        let mut db = LootDatabase::open_in_memory().unwrap();

        // Insert drops from different characters
        let mut player1 = create_test_player();
        player1.char_id = 1111;
        let drop1 = NewLootDrop {
            timestamp: 1700000000000,
            player: player1,
            bag_type: LootBagType::White,
            mob_type: 100,
            mob_name: "Boss".to_string(),
            items: vec![],
        };
        db.insert_loot_drop(&drop1).unwrap();

        let mut player2 = create_test_player();
        player2.char_id = 2222;
        let drop2 = NewLootDrop {
            timestamp: 1700000001000,
            player: player2,
            bag_type: LootBagType::White,
            mob_type: 100,
            mob_name: "Boss".to_string(),
            items: vec![],
        };
        db.insert_loot_drop(&drop2).unwrap();

        // Filter by char_id
        let mut filters = SourceFilters::default();
        filters.set_char(1111, "Knight".to_string(), 768);
        let drops = db
            .get_drops_filtered(0, i64::MAX, &filters, 0, 100)
            .unwrap();
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].char_id, 1111);
    }

    #[test]
    fn test_filtered_query_combined_filters() {
        let mut db = LootDatabase::open_in_memory().unwrap();

        // Insert 4 drops with different combinations
        let mut player = create_test_player();
        player.char_id = 1111;
        player.dungeon = "Sanctuary".to_string();
        let drop1 = NewLootDrop {
            timestamp: 1700000000000,
            player,
            bag_type: LootBagType::White,
            mob_type: 100,
            mob_name: "Oryx 3".to_string(),
            items: vec![],
        };
        db.insert_loot_drop(&drop1).unwrap();

        let mut player = create_test_player();
        player.char_id = 1111;
        player.dungeon = "The Void".to_string();
        let drop2 = NewLootDrop {
            timestamp: 1700000001000,
            player,
            bag_type: LootBagType::White,
            mob_type: 200,
            mob_name: "Void Entity".to_string(),
            items: vec![],
        };
        db.insert_loot_drop(&drop2).unwrap();

        let mut player = create_test_player();
        player.char_id = 2222;
        player.dungeon = "Sanctuary".to_string();
        let drop3 = NewLootDrop {
            timestamp: 1700000002000,
            player,
            bag_type: LootBagType::White,
            mob_type: 100,
            mob_name: "Oryx 3".to_string(),
            items: vec![],
        };
        db.insert_loot_drop(&drop3).unwrap();

        // Filter: Oryx 3 drops in Sanctuary by char 1111
        let mut filters = SourceFilters::default();
        filters.set_mob(100, "Oryx 3".to_string());
        filters.set_dungeon("Sanctuary".to_string());
        filters.set_char(1111, "Knight".to_string(), 768);

        let drops = db
            .get_drops_filtered(0, i64::MAX, &filters, 0, 100)
            .unwrap();
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].mob_type, 100);
        assert_eq!(drops[0].dungeon, "Sanctuary");
        assert_eq!(drops[0].char_id, 1111);
    }

    fn insert_ice_tomb_bag(
        db: &mut LootDatabase,
        seed: i32,
        bag: LootBagType,
        mob_type: i32,
        mob_name: &str,
        items: Vec<(i32, &str)>,
    ) -> i64 {
        let mut player = create_test_player();
        player.dungeon = super::super::ICE_TOMB_NAME.to_string();
        player.map_seed = seed;
        db.insert_loot_drop(&NewLootDrop {
            timestamp: 1700000000000,
            player,
            bag_type: bag,
            mob_type,
            mob_name: mob_name.to_string(),
            items: items
                .into_iter()
                .enumerate()
                .map(|(slot, (item_id, item_name))| NewLootItem {
                    slot: slot as i32,
                    item_id,
                    item_name: item_name.to_string(),
                    enchant_ids: vec![],
                })
                .collect(),
        })
        .unwrap()
    }

    #[test]
    fn migration_v15_attributes_ice_tomb_soul_bags() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        // Two unique rings seed Frimar and Glacius; the ambiguous shared-loot bag
        // is then distributed to the only boss left, Polaris (whose id is known
        // without game assets loaded).
        let ring_f = insert_ice_tomb_bag(
            &mut db,
            5555,
            LootBagType::White,
            0,
            "Unknown",
            vec![(32724, "Frimarra")],
        );
        let ring_g = insert_ice_tomb_bag(
            &mut db,
            5555,
            LootBagType::White,
            0,
            "Unknown",
            vec![(32723, "Ring of the Northern Light")],
        );
        let ambiguous = insert_ice_tomb_bag(
            &mut db,
            5555,
            LootBagType::White,
            0,
            "Unknown",
            vec![(5139, "Freezing Quiver")],
        );

        db.migrate(14).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        let f = |id: i64| drops.iter().find(|d| d.id == id).unwrap();
        // The unique rings name Frimar and Glacius outright, so the ambiguous
        // bag lands on the only boss left -- Polaris. Each is stored as its real
        // boss object id so the loot history draws the boss sprite.
        assert_eq!(
            f(ring_f).mob_type,
            super::super::ICE_TOMB_FRIMAR_OBJECT_TYPE
        );
        assert_eq!(f(ring_f).mob_name, "Frimar");
        assert_eq!(
            f(ring_g).mob_type,
            super::super::ICE_TOMB_GLACIUS_OBJECT_TYPE
        );
        assert_eq!(f(ring_g).mob_name, "Glacius");
        assert_eq!(
            f(ambiguous).mob_type,
            super::super::ICE_TOMB_POLARIS_OBJECT_TYPE
        );
        assert_eq!(f(ambiguous).mob_name, "Polaris");
    }

    #[test]
    fn migration_v15_leaves_non_boss_unknown_bags() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        // An Unknown bag with no Ice Tomb boss loot is not forced onto a boss.
        let junk = insert_ice_tomb_bag(
            &mut db,
            5555,
            LootBagType::White,
            0,
            "Unknown",
            vec![(999_999, "Junk")],
        );
        // Brown bags are never boss drops even if they hold boss loot.
        let brown = insert_ice_tomb_bag(
            &mut db,
            5555,
            LootBagType::Brown,
            0,
            "Unknown",
            vec![(5139, "Freezing Quiver")],
        );

        db.migrate(14).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        let f = |id: i64| drops.iter().find(|d| d.id == id).unwrap();
        assert_eq!(f(junk).mob_type, 0);
        assert_eq!(f(brown).mob_type, 0);
    }

    #[test]
    fn migration_v15_leaves_seed_zero_ambiguous_bags() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        // Seed 0 merges distinct runs, so ambiguous distribution must not fire;
        // a unique ring (Polaris) still resolves though.
        let ambiguous = insert_ice_tomb_bag(
            &mut db,
            0,
            LootBagType::White,
            0,
            "Unknown",
            vec![(5139, "Freezing Quiver")],
        );
        let ring = insert_ice_tomb_bag(
            &mut db,
            0,
            LootBagType::White,
            0,
            "Unknown",
            vec![(32722, "Enchanted Ice Shard")],
        );

        db.migrate(14).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        let f = |id: i64| drops.iter().find(|d| d.id == id).unwrap();
        assert_eq!(f(ambiguous).mob_type, 0);
        assert_eq!(f(ring).mob_type, super::super::ICE_TOMB_POLARIS_OBJECT_TYPE);
        assert_eq!(f(ring).mob_name, "Polaris");
    }

    #[test]
    fn migration_v16_remaps_ice_tomb_soul_ids_to_boss_ids() {
        let mut db = LootDatabase::open_in_memory().unwrap();
        // A row fixed by an earlier build stored the soul id as mob_type, which
        // drew the soul sprite. v16 remaps it onto the real boss object id.
        let bag = insert_ice_tomb_bag(
            &mut db,
            5555,
            LootBagType::White,
            super::super::ICE_TOMB_SUPPORT_SOUL_OBJECT_TYPE,
            "Polaris",
            vec![(5139, "Freezing Quiver")],
        );

        db.migrate(15).unwrap();

        let drops = db.get_recent_drops(10).unwrap();
        let f = |id: i64| drops.iter().find(|d| d.id == id).unwrap();
        assert_eq!(f(bag).mob_type, super::super::ICE_TOMB_POLARIS_OBJECT_TYPE);
        assert_eq!(f(bag).mob_name, "Polaris");
    }
}

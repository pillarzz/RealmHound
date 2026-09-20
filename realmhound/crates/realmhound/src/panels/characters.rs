//! Characters Panel - displays all characters in a grid layout.
//!
//! This panel shows character inventory data from:
//! 1. Cached data (loaded on startup)
//! 2. HTTP API (manual refresh when game is closed)
//! 3. Real-time NEWTICK packets (for the current character)

use crate::ui_ext::HoverTooltipExt;
use eframe::egui::{self, Color32, RichText, ScrollArea};
use realmhound_core::{
    api::{
        debug_order_alignment, decode_pcstats, parse_account_data, parse_char_list, AccountData,
        CharacterStats, ClassExaltation, RotmgApiClient,
    },
    assets::get_dungeon_portal_map,
    stats::{
        biome_first_tier_bonus, biome_kill_bonus, biome_tier_breakdown, collection_bonus,
        dead_fame_total, dungeon_awards_fame, dungeon_completion_bonus, dungeon_first_tier_bonus,
        dungeon_tier_breakdown, dungeon_tracked, get_dungeon_list, stat_first_tier_bonus,
        stat_line_bonus, stat_tier_breakdown, FameTierBreakdown, DUNGEON_COLLECTIONS,
        UNTRACKED_DUNGEON_TOOLTIP,
    },
    vault::{CachedCharacter, CharacterCache},
};
use std::collections::HashMap;
use std::sync::mpsc;
use std::thread;

use crate::panels::character_card::{
    format_fame, CharacterCardWidget, StatDisplay, CARD_CONTENT_WIDTH, CARD_PADDING, CARD_SPACING,
    FAME_SPRITE_ID,
};
use crate::panels::derived_stats;
use crate::panels::exalt_view::{exalt_colors, ExaltSurfaces};
use crate::panels::missions::attach_dungeon_tooltip;
use crate::panels::{ActiveTab, AppAction, Panel, PanelContext};
use crate::rendering::{EmbeddedIcon, SpriteRenderer};
use crate::shadcn_ui::Shadcn;
use crate::tab_icons::{
    get_character_subtab_icon, get_graveyard_subtab_icon, grave_sprite_for_char,
};

/// Leading gutter (px) reserved in a fame cell for the greyed "+" prefix on
/// predicted rows, so it sits inside the fame column instead of overlapping the
/// value column and stays aligned with the earned rows' fame icon.
const FAME_PLUS_GUTTER: f32 = 11.0;

/// Biome display names, in the order shown in the BIOME ENEMY KILLS column.
/// Used to size the MISC STATS label cell so its value column lines up with
/// the biome kill counts above it.
const BIOME_DISPLAY_NAMES: [&str; 20] = [
    "Ruins",
    "Beach",
    "Undead Forest",
    "Forest",
    "Plains",
    "Wither",
    "Dark Forest",
    "Desert",
    "Coral Reefs",
    "Sprite Forest",
    "Haunted Hallows",
    "Shipwreck Cove",
    "Dead Church",
    "Risen Hells",
    "Abandoned City",
    "Deep Sea Abyss",
    "Carboniferous",
    "Floral Escape",
    "Sanguine Forest",
    "Runic Tundra",
];

/// Maximum length (in characters) of a custom character label. Caps the label
/// so a long name can't stretch the fixed-width character card and break the
/// grid layout. Tune this to taste.
const MAX_LABEL_LEN: usize = 15;

/// One autocomplete suggestion for the character search box, carrying the icon
/// data needed to render it: class suggestions show the default class sprite,
/// while ID/label suggestions show the matching character's dyed skin.
enum SearchSuggestion {
    Class {
        name: String,
        class_id: i32,
    },
    Character {
        text: String,
        skin_id: i32,
        tex1: u32,
        tex2: u32,
    },
}

impl SearchSuggestion {
    /// Text inserted into the search box when this suggestion is chosen.
    fn text(&self) -> &str {
        match self {
            SearchSuggestion::Class { name, .. } => name,
            SearchSuggestion::Character { text, .. } => text,
        }
    }
}

/// Truncate a stored label to `MAX_LABEL_LEN` characters so legacy labels saved
/// before the limit was introduced can't stretch the fixed-width card.
fn truncate_label(label: String) -> String {
    if label.chars().count() > MAX_LABEL_LEN {
        label.chars().take(MAX_LABEL_LEN).collect()
    } else {
        label
    }
}

/// Top-level character tabs (regular vs seasonal vs graveyard).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharacterTab {
    Regular,
    Seasonal,
    Graveyard,
}

/// Card sort order for the Characters toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CharacterSort {
    /// User-defined drag order (the only mode where dragging is enabled).
    #[default]
    Custom,
    FameDesc,
    FameAsc,
    MaxedDesc,
    MaxedAsc,
    /// Class name A-Z, then by char id within each class.
    Class,
    IdAsc,
    IdDesc,
}

impl CharacterSort {
    /// Serialize to the settings string key.
    pub fn as_key(self) -> &'static str {
        match self {
            CharacterSort::Custom => "custom",
            CharacterSort::FameDesc => "fame_desc",
            CharacterSort::FameAsc => "fame_asc",
            CharacterSort::MaxedDesc => "maxed_desc",
            CharacterSort::MaxedAsc => "maxed_asc",
            CharacterSort::Class => "class",
            CharacterSort::IdAsc => "id_asc",
            CharacterSort::IdDesc => "id_desc",
        }
    }

    /// Parse from a settings string key (defaults to `Custom`).
    pub fn from_key(key: &str) -> Self {
        match key {
            "fame_desc" => CharacterSort::FameDesc,
            "fame_asc" => CharacterSort::FameAsc,
            "maxed_desc" => CharacterSort::MaxedDesc,
            "maxed_asc" => CharacterSort::MaxedAsc,
            "class" => CharacterSort::Class,
            "id_asc" => CharacterSort::IdAsc,
            "id_desc" => CharacterSort::IdDesc,
            _ => CharacterSort::Custom,
        }
    }

    /// Short label for the toolbar dropdown.
    pub fn label(self) -> &'static str {
        match self {
            CharacterSort::Custom => "Custom",
            CharacterSort::FameDesc => "Fame ↓",
            CharacterSort::FameAsc => "Fame ↑",
            CharacterSort::MaxedDesc => "Stats maxed ↓",
            CharacterSort::MaxedAsc => "Stats maxed ↑",
            CharacterSort::Class => "By class (A-Z)",
            CharacterSort::IdAsc => "By ID ↑",
            CharacterSort::IdDesc => "By ID ↓",
        }
    }

    /// Tooltip text shown on hover in the sort dropdown.
    pub fn tooltip(self) -> &'static str {
        match self {
            CharacterSort::Custom => "Re-arrange your characters by dragging their cards around",
            CharacterSort::IdAsc | CharacterSort::IdDesc => {
                "Re-arranging characters is disabled in this mode. Lower ID indicates older character creation date"
            }
            _ => "Re-arranging characters is disabled in this mode",
        }
    }

    /// Whether drag-reordering is allowed in this sort mode.
    pub fn allows_drag(self) -> bool {
        matches!(self, CharacterSort::Custom)
    }

    /// All modes in dropdown order.
    pub fn all() -> [CharacterSort; 8] {
        [
            CharacterSort::Custom,
            CharacterSort::FameDesc,
            CharacterSort::FameAsc,
            CharacterSort::MaxedDesc,
            CharacterSort::MaxedAsc,
            CharacterSort::Class,
            CharacterSort::IdAsc,
            CharacterSort::IdDesc,
        ]
    }
}

/// Actions that can be triggered from the characters panel.
#[derive(Debug, Clone)]
pub enum CharacterAction {
    /// View loot history for a character (char_id, char_name, icon_id).
    ViewLoot(i32, String, i32),
    /// View boss fight history for a character (char_id, char_name).
    ViewFights(i32, String),
}

/// Icon mappings for statistics (stat_name -> object_id).
/// Object IDs are from ObjectID.list. None means TBD/no icon.
fn get_stat_icon(stat_name: &str) -> Option<i32> {
    match stat_name {
        "Shots Fired" => Some(3073),
        "Hits" => Some(8963),
        "Ability Uses" => Some(37957),
        "Tiles Discovered" => Some(24109),
        "Teleports" => Some(2787),
        "Potions Drunk" => Some(2594),
        "Kills" => Some(1736),
        "Assists" => Some(1736),
        "Party Level-ups" => Some(30942),
        "Lesser Gods Kills" => Some(24137),
        "Encounter Kills" => Some(3412),
        "Hero Kills" => Some(21897),
        "Critter Kills" => Some(21845),
        "Beast Kills" => Some(21842),
        "Humanoid Kills" => Some(21780),
        "Undead Kills" => Some(34586),
        "Nature Kills" => Some(3331),
        "Construct Kills" => Some(2491),
        "Grotesque Kills" => Some(21862),
        "Structure Kills" => Some(3369),
        "God Kills" => Some(21889),
        "Assists Against Gods" => Some(21889),
        "Cube Kills" => Some(22009),
        "Oryx Kills" => Some(5952),
        "Quests Completed" => Some(21843),
        "Minutes Active" => Some(788),
        "Dungeon Types Completed" => None, // Uses embedded icon (Dungeon Types Completed.png)
        "Stat Potions Consumed" => Some(5094),
        _ => None,
    }
}

/// Icon mappings for biome kills (biome_name -> beacon object_id).
/// Object IDs are from ObjectID.list (beacon sprites in beacons32x32 sheet).
fn get_biome_icon(biome_name: &str) -> Option<i32> {
    match biome_name {
        "Beach" | "Shore" => Some(9840),                  // Shores Beacon
        "Ruins" | "Forest" | "Dark Forest" => Some(9841), // Nature Ruins Beacon
        "Undead Forest" => Some(9842),                    // Gloomy Beacon
        "Desert" => Some(9843),                           // Arid Beacon
        "Plains" | "Wither" => Some(9844),                // Plains Beacon
        "Abandoned City" => Some(9845),                   // Abandoned Beacon
        "Coral Reefs" => Some(9846),                      // Oceanic Beacon
        "Dead Church" => Some(9847),                      // Gothic Beacon
        "Haunted Hallows" => Some(9848),                  // Haunted Beacon
        "Risen Hells" => Some(9849),                      // Hell Beacon
        "Shipwreck Cove" => Some(9850),                   // Shipwrecked Beacon
        "Sprite Forest" => Some(9851),                    // Fey Beacon
        "Carboniferous" => Some(9852),                    // Prehistoric Beacon
        "Deep Sea Abyss" => Some(9853),                   // Abyssal Beacon
        "Floral Escape" => Some(9854),                    // Floral Beacon
        "Sanguine Forest" => Some(9855),                  // Sanguine Beacon
        "Runic Tundra" => Some(9856),                     // Frozen Beacon
        _ => None,
    }
}

/// State of the API fetch operation.
#[derive(Debug, Clone)]
pub enum CharactersFetchState {
    /// Ready (no fetch in progress)
    Idle,
    /// Waiting for API response
    Loading,
    /// Error occurred
    Error(String),
}

/// Sort order for the DUNGEONS list on the character stats page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DungeonSort {
    /// In-game default order (ALL_DUNGEONS).
    #[default]
    Default,
    /// Easiest to hardest (unknown difficulty last).
    DifficultyAsc,
    /// Hardest to easiest (unknown difficulty last).
    DifficultyDesc,
    /// Highest completion count first.
    MostCompleted,
}

impl DungeonSort {
    fn as_key(self) -> &'static str {
        match self {
            DungeonSort::Default => "default",
            DungeonSort::DifficultyAsc => "difficulty_asc",
            DungeonSort::DifficultyDesc => "difficulty_desc",
            DungeonSort::MostCompleted => "most_completed",
        }
    }

    fn from_key(key: &str) -> Self {
        match key {
            "difficulty_asc" => DungeonSort::DifficultyAsc,
            "difficulty_desc" => DungeonSort::DifficultyDesc,
            "most_completed" => DungeonSort::MostCompleted,
            _ => DungeonSort::Default,
        }
    }
}

/// Data for the Current-Character widget-bar chip and its Attributes tooltip.
pub struct CurrentCharWidget {
    /// Sprite id to draw (skin id when skinned, else class id).
    pub sprite_id: i32,
    /// Clothing dye texture (StatType::Texture1) for the dyed sprite.
    pub tex1: u32,
    /// Accessory dye texture (StatType::Texture2) for the dyed sprite.
    pub tex2: u32,
    /// Display label (custom label, else class name).
    pub label: String,
    /// Derived Attributes / DPS / regen readout.
    pub derived: realmhound_core::stats::derived::DerivedStats,
    /// Per-stat maxed flags `[HP, MP, ATT, DEF, SPD, DEX, VIT, WIS]`.
    pub maxed: [bool; 8],
    /// Live loot-boost / crucible context for the Bonuses section.
    pub bonus: derived_stats::BonusContext,
    /// Character identity + fame lines folded into the Attributes readout
    /// (matches the Character Stats page).
    pub identity: derived_stats::StatsIdentity,
}

/// Nominal column width unit (px) used to decide how many columns fit the
/// window and as the width of an empty (drop-target) column.
const STATS_COL_WIDTH: f32 = 280.0;
/// Minimum width (px) a non-empty column may shrink to. A column with only
/// narrow cards (e.g. MISC STATS) collapses toward its content down to this
/// floor instead of padding out to the full nominal column width.
const STATS_MIN_CARD_W: f32 = 140.0;
/// Stable indices of the Character-Stats blocks, in flow order.
const STATS_BLOCK_ATTRIBUTES: usize = 0;
const STATS_BLOCK_MISC: usize = 1;
const STATS_BLOCK_COLLECTIONS: usize = 2;
const STATS_BLOCK_DUNGEONS_0: usize = 3;
const STATS_BLOCK_DUNGEONS_1: usize = 4;
const STATS_BLOCK_BIOMES: usize = 5;
const STATS_BLOCK_STATISTICS: usize = 6;
const STATS_BLOCK_COUNT: usize = 7;

/// Stable indices of the Character-Stats categories shown in the "Show
/// categories" dropdown. DUNGEONS covers both dungeon half-blocks.
const CAT_ATTRIBUTES: usize = 0;
const CAT_MISC: usize = 1;
const CAT_COLLECTIONS: usize = 2;
const CAT_DUNGEONS: usize = 3;
const CAT_BIOMES: usize = 4;
const CAT_STATISTICS: usize = 5;
const CAT_COUNT: usize = 6;

/// Category label for each visibility toggle, indexed by `CAT_*`.
const CATEGORY_LABELS: [&str; CAT_COUNT] = [
    "ATTRIBUTES",
    "MISC STATS",
    "DUNGEON COLLECTIONS",
    "DUNGEONS",
    "BIOME ENEMY KILLS",
    "STATISTICS",
];

/// Map a stats block (STATS_BLOCK_*) to the category (CAT_*) that controls its
/// visibility.
fn block_category(block: usize) -> usize {
    match block {
        STATS_BLOCK_ATTRIBUTES => CAT_ATTRIBUTES,
        STATS_BLOCK_MISC => CAT_MISC,
        STATS_BLOCK_COLLECTIONS => CAT_COLLECTIONS,
        STATS_BLOCK_DUNGEONS_0 | STATS_BLOCK_DUNGEONS_1 => CAT_DUNGEONS,
        STATS_BLOCK_BIOMES => CAT_BIOMES,
        _ => CAT_STATISTICS,
    }
}

/// Characters panel state.
pub struct CharactersPanel {
    /// Character cache (loaded from disk, updated from API/NEWTICK)
    cache: CharacterCache,
    /// Current fetch state
    fetch_state: CharactersFetchState,
    /// Receiver for API results (returns AccountData which includes characters and exaltation stats)
    api_result_rx: Option<mpsc::Receiver<Result<AccountData, String>>>,
    /// Token (and capture time) used for the last fetch, so the app can persist
    /// exactly the token a successful fetch verified (not a mid-flight swap).
    last_fetch_token: Option<(String, Option<chrono::DateTime<chrono::Utc>>)>,
    /// Currently playing character ID (from CREATE_SUCCESS)
    live_char_id: Option<i32>,
    /// Last-known loot-drop-boost seconds per character id. Populated from the
    /// API refresh (`<LDTimer>` on each `<Char>`) and updated live from packets
    /// for the currently playing character. A missing entry means no active
    /// boost is known for that character.
    loot_boost_by_char: std::collections::HashMap<i32, u32>,
    /// Whether a character label is being edited
    editing_label: Option<i32>,
    /// Persistent open state for the label edit dialog. Kept in sync with the
    /// dialog so its internal "last-open" tracking stays correct across opens.
    label_dialog_open: bool,
    /// Focus the label text field only on the first frame the dialog opens.
    label_dialog_focus: bool,
    /// Text buffer for label editing
    label_edit_buffer: String,
    /// Active tab (regular vs seasonal)
    active_tab: CharacterTab,
    /// Character ID whose stats view is open (None = show character list)
    viewing_stats_for: Option<i32>,
    /// Frame count when stats modal was opened (used to reset scroll)
    stats_modal_opened_frame: u64,
    /// Character ID currently being dragged (for reordering)
    dragging_char_id: Option<i32>,
    /// Index where drop would occur (for visual indicator)
    drop_target_index: Option<usize>,
    /// Position where context menu was opened (for showing at click location)
    context_menu_pos: Option<egui::Pos2>,
    /// Character ID whose context menu is open (None = no menu)
    context_menu_char_id: Option<i32>,
    /// Pending action to return from render
    pending_action: Option<CharacterAction>,
    /// Cached count of regular characters (updated on cache changes)
    cached_regular_count: usize,
    /// Cached count of seasonal characters (updated on cache changes)
    cached_seasonal_count: usize,
    /// Cached count of deceased characters (graveyard)
    cached_dead_count: usize,
    /// Exaltation stats per class (class_type -> ClassExaltation)
    exaltation_stats: HashMap<i32, ClassExaltation>,
    /// Flag indicating cache was modified and needs to be synced to AccountData
    needs_save: bool,
    /// UI-origin edits (label/remove/move) to route to the processor.
    pending_edits: Vec<AppAction>,
    /// Sort order for the DUNGEONS list on the stats page.
    dungeon_sort: DungeonSort,
    /// Card sort mode for the character grid (toolbar).
    sort_mode: CharacterSort,
    /// Stats-maxed section display mode applied to all cards.
    stat_display: StatDisplay,
    /// Whether the main inventory section is shown on cards.
    show_inventory: bool,
    /// Whether the backpack section is shown on cards.
    show_backpack: bool,
    /// Whether the backpack-extender section is shown on cards.
    show_extender: bool,
    /// Whether the potion-belt section is shown on cards.
    show_belt: bool,
    /// Free-text search filter (class / ID / label) for the character grid.
    search_query: String,
    /// Whether the search autocomplete popup is open.
    search_popup_open: bool,
    /// Highlighted suggestion index for keyboard navigation.
    search_selected_index: Option<usize>,
    /// Target defense used by the theoretical-DPS section on the stats page.
    theoretical_target_def: i32,
    /// Live edit buffer for the Enemy-DEF input, shared by the stats page and
    /// the Current-Character hover widget so both always show the same number.
    target_def_text: String,
    /// Width (px) of each Character-Stats block measured last frame, indexed by
    /// STATS_BLOCK_*. Used to size each column to its widest card's content
    /// width, measured a frame behind.
    stats_block_widths: [f32; STATS_BLOCK_COUNT],
    /// Which column (0-based) each Character-Stats block is assigned to, indexed
    /// by STATS_BLOCK_*. The user drags blocks between columns; there is no
    /// automatic space-filling flow. Within a column, blocks render in
    /// `stats_block_order` sequence.
    stats_block_column: [usize; STATS_BLOCK_COUNT],
    /// Per-category visibility for the stats page ("Show categories" dropdown),
    /// indexed by CAT_*. Hidden categories drop their header + content entirely
    /// and the remaining blocks re-flow into the freed space. All on by default.
    category_visible: [bool; CAT_COUNT],
    /// User-defined order of the stats blocks (indexed positions hold
    /// STATS_BLOCK_* ids). Dragging a category card by its edge reorders this
    /// sequence, then the flow re-packs it into columns by height.
    stats_block_order: [usize; STATS_BLOCK_COUNT],
    /// Stats block currently being drag-reordered by its card edge (if any).
    stats_dragging_block: Option<usize>,
    /// Whether the ATTRIBUTES card shows the Gear/Exalts/Crucible breakdown
    /// columns ("Show stats breakdown" toggle). Off by default -- only the
    /// bright Total column shows and the card shrinks to that width.
    stats_show_breakdown: bool,
}

impl CharactersPanel {
    /// Create a new characters panel with an existing character cache.
    ///
    /// The cache is provided by AccountData to ensure single source of truth.
    pub fn with_cache(cache: CharacterCache) -> Self {
        tracing::info!(
            "[CHARACTERS] Initialized with {} cached characters, {} exaltation classes",
            cache.characters.len(),
            cache.exaltation_stats.len()
        );

        // Compute initial tab counts
        let cached_regular_count = cache.characters.iter().filter(|c| !c.seasonal).count();
        let cached_seasonal_count = cache.characters.iter().filter(|c| c.seasonal).count();
        let cached_dead_count = cache.dead_characters.len();

        // Load exaltation stats from cache
        let exaltation_stats = cache.exaltation_stats.clone();

        Self {
            cache,
            fetch_state: CharactersFetchState::Idle,
            api_result_rx: None,
            last_fetch_token: None,
            live_char_id: None,
            loot_boost_by_char: std::collections::HashMap::new(),
            editing_label: None,
            label_dialog_open: false,
            label_dialog_focus: false,
            label_edit_buffer: String::new(),
            active_tab: CharacterTab::Seasonal,
            viewing_stats_for: None,
            stats_modal_opened_frame: 0,
            dragging_char_id: None,
            drop_target_index: None,
            context_menu_pos: None,
            context_menu_char_id: None,
            pending_action: None,
            cached_regular_count,
            cached_seasonal_count,
            cached_dead_count,
            exaltation_stats,
            needs_save: false,
            pending_edits: Vec::new(),
            dungeon_sort: DungeonSort::default(),
            sort_mode: CharacterSort::default(),
            stat_display: StatDisplay::default(),
            show_inventory: true,
            show_backpack: true,
            show_extender: true,
            show_belt: true,
            search_query: String::new(),
            search_popup_open: false,
            search_selected_index: None,
            theoretical_target_def: 0,
            target_def_text: "0".to_string(),
            // Seed with a nominal width so the first frame lays out before real
            // content widths are measured.
            stats_block_widths: [280.0; STATS_BLOCK_COUNT],
            // Default column layout: col0 ATTRIBUTES+MISC, col1 COLLECTIONS,
            // col2/col3 the two DUNGEONS halves, col4 BIOMES+STATISTICS.
            stats_block_column: [0, 0, 1, 2, 3, 4, 4],
            category_visible: [true; CAT_COUNT],
            stats_block_order: [0, 1, 2, 3, 4, 5, 6],
            stats_dragging_block: None,
            stats_show_breakdown: false,
        }
    }

    /// Apply persisted toolbar settings after construction.
    pub fn apply_settings(&mut self, settings: &realmhound_core::settings::CharactersSettings) {
        if let Some(key) = &settings.sort_mode {
            self.sort_mode = CharacterSort::from_key(key);
        }
        if let Some(key) = &settings.stat_display {
            self.stat_display = StatDisplay::from_key(key);
        }
        self.show_inventory = settings.show_inventory;
        self.show_backpack = settings.show_backpack;
        self.show_extender = settings.show_extender;
        self.show_belt = settings.show_belt;

        // Stats-page layout: restore only when the persisted vectors match the
        // current block/category counts (guards against schema drift).
        if let Some(order) = &settings.stats_block_order {
            if order.len() == STATS_BLOCK_COUNT && order.iter().all(|&i| i < STATS_BLOCK_COUNT) {
                for (dst, &src) in self.stats_block_order.iter_mut().zip(order.iter()) {
                    *dst = src;
                }
            }
        }
        if let Some(cols) = &settings.stats_block_column {
            if cols.len() == STATS_BLOCK_COUNT {
                for (dst, &src) in self.stats_block_column.iter_mut().zip(cols.iter()) {
                    *dst = src;
                }
            }
        }
        if let Some(vis) = &settings.stats_category_visible {
            if vis.len() == CAT_COUNT {
                for (dst, &src) in self.category_visible.iter_mut().zip(vis.iter()) {
                    *dst = src;
                }
            }
        }
        if let Some(key) = &settings.stats_dungeon_sort {
            self.dungeon_sort = DungeonSort::from_key(key);
        }
        self.stats_show_breakdown = settings.stats_show_breakdown;
    }

    /// Snapshot the current toolbar settings for persistence.
    fn to_settings(&self) -> realmhound_core::settings::CharactersSettings {
        realmhound_core::settings::CharactersSettings {
            sort_mode: Some(self.sort_mode.as_key().to_string()),
            stat_display: Some(self.stat_display.as_key().to_string()),
            show_inventory: self.show_inventory,
            show_backpack: self.show_backpack,
            show_extender: self.show_extender,
            show_belt: self.show_belt,
            // Owned by the app (persisted in refresh_snapshot); preserved by the
            // SaveCharactersSettings handler so this snapshot doesn't clobber it.
            last_live_char_id: None,
            stats_block_order: Some(self.stats_block_order.to_vec()),
            stats_block_column: Some(self.stats_block_column.to_vec()),
            stats_category_visible: Some(self.category_visible.to_vec()),
            stats_dungeon_sort: Some(self.dungeon_sort.as_key().to_string()),
            stats_show_breakdown: self.stats_show_breakdown,
        }
    }

    /// Observe the live-packet loot-boost value (mirrored from `ViewState`
    /// every frame) for the currently playing character. Live packets are
    /// authoritative, so this writes the value into the per-character map for
    /// the live character (removing the entry when the boost ends). When no
    /// character is live the map keeps each card's last-known value.
    pub fn observe_live_loot_boost(&mut self, live: Option<u32>, live_char_id: Option<i32>) {
        if let Some(id) = live_char_id {
            match live {
                Some(secs) => {
                    self.loot_boost_by_char.insert(id, secs);
                }
                None => {
                    self.loot_boost_by_char.remove(&id);
                }
            }
        }
    }

    /// Build the Current-Character widget data for `char_id`: the (dyed) sprite
    /// id + dye textures, a display label, the derived Attributes readout, and
    /// the live loot-boost / crucible context for its Bonuses section. Returns
    /// `None` when the character isn't in the cache.
    pub fn current_char_widget(&self, char_id: i32, is_live: bool) -> Option<CurrentCharWidget> {
        let char = self.cache.find_any_character(char_id)?.clone();
        let derived = derived_stats::compute(&char, &self.cache);
        let sprite_id = if char.skin > 0 {
            char.skin
        } else {
            char.class_id as i32
        };
        let label = self
            .cache
            .get_label(char_id)
            .map(|s| truncate_label(s.to_string()))
            .unwrap_or_else(|| char.class_name().to_string());
        let loot_boost_active = self.loot_boost_by_char.get(&char_id).copied().unwrap_or(0) > 0;
        let bonus = derived_stats::build_context(&char, &self.cache, loot_boost_active, is_live);
        let class_name = char.class_name();
        let stats = decode_pcstats(&char.pc_stats_raw);
        let fame_on_death: Option<i64> = if char.is_dead && char.death_fame > 0 {
            Some(char.death_fame as i64)
        } else {
            stats
                .as_ref()
                .map(|s| dead_fame_total(char.fame, s, char.maxed_flags(), DUNGEON_COLLECTIONS))
        };
        let identity = derived_stats::StatsIdentity {
            skin_id: sprite_id,
            tex1: char.tex1,
            tex2: char.tex2,
            fallback_initial: class_name.chars().next().unwrap_or('?'),
            is_dead: char.is_dead,
            grave: if char.is_dead {
                Some(grave_sprite_for_char(
                    char.gravestone_type,
                    char.maxed_count,
                    char.level,
                ))
            } else {
                None
            },
            title: format!("{} Lv.{}", label, char.level),
            subtitle: format!(
                "#{} • {} • {}/8",
                char.char_id, class_name, char.maxed_count
            ),
            base_fame: char.fame,
            killed_by: if char.is_dead {
                Some(char.killed_by.as_deref().unwrap_or("Unknown").to_string())
            } else {
                None
            },
            killer_icon: if char.is_dead {
                realmhound_core::assets::get_asset_manager()
                    .killer_sprite_id(char.killed_by.as_deref().unwrap_or("Unknown"))
            } else {
                None
            },
            fame_on_death,
        };
        Some(CurrentCharWidget {
            sprite_id,
            tex1: char.tex1,
            tex2: char.tex2,
            label,
            derived,
            maxed: char.maxed_flags(),
            bonus,
            identity,
        })
    }

    /// Shared Enemy-DEF value + edit buffer for the weapon DPS-vs-defense line,
    /// synced across the Current-Character widget tooltip and every stats-page
    /// section.
    pub fn target_def_fields_mut(&mut self) -> (&mut i32, &mut String) {
        (&mut self.theoretical_target_def, &mut self.target_def_text)
    }

    /// Whether the ATTRIBUTES readout should show the Gear/Exalts/Crucible
    /// breakdown columns. The Current-Character widget tooltip mirrors this so it
    /// stays in sync with the "Show stats breakdown" toggle on the stats page.
    pub fn stats_show_breakdown(&self) -> bool {
        self.stats_show_breakdown
    }

    /// Resync this mirror panel from the processor-owned canonical cache.
    ///
    /// Replaces the display cache, live-character highlight, and exaltation
    /// stats from the snapshot, then recomputes tab counts. Does not mark the
    /// cache dirty (the processor owns persistence).
    pub fn sync_from(&mut self, cache: &CharacterCache, live_char_id: Option<i32>) {
        self.cache = cache.clone();
        self.exaltation_stats = cache.exaltation_stats.clone();
        self.live_char_id = live_char_id;
        self.cached_regular_count = self.cache.characters.iter().filter(|c| !c.seasonal).count();
        self.cached_seasonal_count = self.cache.characters.iter().filter(|c| c.seasonal).count();
        self.cached_dead_count = self.cache.dead_characters.len();
    }

    /// Recompute cached tab counts. Call after adding/removing characters.
    fn recompute_tab_counts(&mut self) {
        self.cached_regular_count = self.cache.characters.iter().filter(|c| !c.seasonal).count();
        self.cached_seasonal_count = self.cache.characters.iter().filter(|c| c.seasonal).count();
        self.cached_dead_count = self.cache.dead_characters.len();
        self.needs_save = true;
    }

    /// Mark the character cache as modified so it is synced to `AccountData`
    /// on the next save. Call after any cache mutation that doesn't go through
    /// `recompute_tab_counts()`.
    fn mark_needs_save(&mut self) {
        self.needs_save = true;
    }

    /// Mark a character as dead, moving it into the graveyard.
    pub fn mark_dead(
        &mut self,
        char_id: i32,
        killed_by: &str,
        death_fame: i32,
        gravestone_type: Option<i32>,
    ) {
        self.cache
            .mark_dead(char_id, killed_by, death_fame, gravestone_type);
        // Clear live char if it's the one that died
        if self.live_char_id == Some(char_id) {
            self.live_char_id = None;
        }
        // Death moves the char between lists, so counts must be recomputed.
        self.recompute_tab_counts();
    }

    /// Remove a dead character from the cache.
    pub fn remove_character(&mut self, char_id: i32) {
        self.cache.remove_character(char_id);
        self.recompute_tab_counts();
        self.pending_edits.push(AppAction::RemoveCharacter(char_id));
    }

    /// Check for completed API fetch.
    ///
    /// Returns the fetched account data on success so the caller can forward it
    /// to the processor (canonical owner). The local display cache is also
    /// updated optimistically to avoid a one-frame flash.
    pub fn check_api_result(&mut self) -> Option<AccountData> {
        if let Some(rx) = &self.api_result_rx {
            match rx.try_recv() {
                Ok(Ok(account_data)) => {
                    tracing::info!(
                        "[CHARACTERS] Loaded {} characters from API, {} exaltation classes",
                        account_data.characters.len(),
                        account_data.exaltation_stats.len()
                    );
                    let applied = self.cache.update_from_api(
                        &account_data.characters,
                        account_data.account_id.as_deref(),
                    );
                    if applied {
                        // Store exaltation stats for display in stats panel
                        self.exaltation_stats = account_data.exaltation_stats.clone();
                        // Also persist to cache for next app startup
                        self.cache.exaltation_stats = account_data.exaltation_stats.clone();
                        // Account-wide accelerators (dust boost) from char/list.
                        self.cache.account_accelerators = account_data.accelerators.clone();
                        // Seed each card's loot-boost from the API snapshot. Live
                        // packets override the playing character afterwards.
                        for c in &account_data.characters {
                            match c.loot_boost_secs {
                                Some(secs) => {
                                    self.loot_boost_by_char.insert(c.char_id, secs);
                                }
                                None => {
                                    self.loot_boost_by_char.remove(&c.char_id);
                                }
                            }
                        }
                    } else {
                        tracing::warn!(
                            "[CHARACTERS] API result rejected (different account); display left unchanged."
                        );
                    }
                    self.recompute_tab_counts();
                    self.fetch_state = CharactersFetchState::Idle;
                    self.api_result_rx = None;
                    return Some(account_data);
                }
                Ok(Err(e)) => {
                    tracing::error!("[CHARACTERS] API error: {}", e);
                    self.fetch_state = CharactersFetchState::Error(e);
                    self.api_result_rx = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    // Still loading
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.fetch_state =
                        CharactersFetchState::Error("API thread disconnected".to_string());
                    self.api_result_rx = None;
                }
            }
        }
        None
    }

    /// Check if a fetch is currently in progress.
    pub fn is_loading(&self) -> bool {
        matches!(self.fetch_state, CharactersFetchState::Loading)
    }

    /// Start fetching character data from the API. `captured_at` only enriches
    /// the message shown if the server rejects the token.
    pub fn fetch_characters(
        &mut self,
        access_token: &str,
        captured_at: Option<chrono::DateTime<chrono::Utc>>,
    ) {
        let token = access_token.to_string();
        let (tx, rx) = mpsc::channel();

        self.last_fetch_token = Some((token.clone(), captured_at));
        self.fetch_state = CharactersFetchState::Loading;
        self.api_result_rx = Some(rx);

        thread::spawn(move || {
            let client = RotmgApiClient::new(token);
            match client.get_char_list() {
                Ok(xml) => {
                    if xml.contains("<Error>Account in use</Error>") {
                        let _ = tx.send(Err(
                            "Account in use - close the game to refresh.".to_string()
                        ));
                        return;
                    }
                    if xml.contains("<Error>") {
                        let _ = tx.send(Err("Server returned an error".to_string()));
                        return;
                    }
                    let result = parse_account_data(&xml).map_err(|e| e.to_string());
                    let _ = tx.send(result);
                }
                Err(e) => {
                    let log_msg = match &e {
                        realmhound_core::api::ApiError::Http(_) => {
                            "HTTP request failed".to_string()
                        }
                        other => format!("{other}"),
                    };
                    tracing::error!("[CHARACTERS] API request failed: {log_msg}");
                    let user_msg = match &e {
                        realmhound_core::api::ApiError::Server(msg)
                            if msg.contains("Account in use") =>
                        {
                            "Account in use - close the game to refresh.".to_string()
                        }
                        realmhound_core::api::ApiError::RateLimited => {
                            "Rate limited - please wait before retrying.".to_string()
                        }
                        realmhound_core::api::ApiError::Auth(_) => {
                            realmhound_core::api::token_expiry_message(captured_at)
                        }
                        _ => "Failed to reload account data - please try again.".to_string(),
                    };
                    let _ = tx.send(Err(user_msg));
                }
            }
        });
    }

    /// The token (and capture time) used for the most recent fetch, if any.
    pub fn last_fetch_token(&self) -> Option<(&str, Option<chrono::DateTime<chrono::Utc>>)> {
        self.last_fetch_token
            .as_ref()
            .map(|(t, at)| (t.as_str(), *at))
    }

    /// The current fetch error message, if the last fetch failed.
    pub fn fetch_error(&self) -> Option<&str> {
        match &self.fetch_state {
            CharactersFetchState::Error(msg) => Some(msg.as_str()),
            _ => None,
        }
    }

    /// On a fresh token: drop any in-flight old-token request and clear errors.
    pub fn reset_on_new_token(&mut self) {
        self.api_result_rx = None;
        if matches!(
            self.fetch_state,
            CharactersFetchState::Error(_) | CharactersFetchState::Loading
        ) {
            self.fetch_state = CharactersFetchState::Idle;
        }
    }

    /// Parse character data from NewCharacterInfo packet XML.
    /// This updates only the characters present in the XML, not replacing the whole list.
    pub fn parse_character_xml(&mut self, xml: &str) {
        match parse_char_list(xml) {
            Ok(characters) => {
                tracing::info!(
                    "[CHARACTERS] Parsed {} characters from packet",
                    characters.len()
                );
                // Update each character individually instead of replacing all
                for character in &characters {
                    self.cache.update_single_character(character);
                }
                self.recompute_tab_counts();
            }
            Err(e) => {
                tracing::error!("[CHARACTERS] Failed to parse character XML: {}", e);
            }
        }
    }

    /// Render the characters panel.
    /// Returns an optional action to perform (like viewing loot for a character).
    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        access_token: &Option<String>,
        sprite_renderer: &mut SpriteRenderer,
        shadcn: &Shadcn,
    ) -> Option<CharacterAction> {
        // API results are polled at the App level (forwarded to the processor),
        // so the panel must not consume them here.

        // If viewing stats for a character, show full-panel stats view
        if self.viewing_stats_for.is_some() {
            self.render_stats_view(ui, sprite_renderer, shadcn);
            return self.pending_action.take();
        }

        // Toolbar band: sort, stat display, and section toggles.
        // Rendered above the Seasonal/Regular/Graveyard tabs.
        let mut settings_changed = false;
        shadcn.header_band(ui, shadcn.secondary_header_fill(), |ui| {
            shadcn.band_row(ui, |ui| {
                ui.label(RichText::new("Sort:").small().color(Color32::GRAY));
                let sort_resp = egui::ComboBox::from_id_salt("characters_sort_mode")
                    .selected_text(self.sort_mode.label())
                    .show_ui(ui, |ui| {
                        for mode in CharacterSort::all() {
                            if ui
                                .selectable_label(self.sort_mode == mode, mode.label())
                                .hover_tip(mode.tooltip())
                                .clicked()
                            {
                                if self.sort_mode != mode {
                                    self.sort_mode = mode;
                                    settings_changed = true;
                                    // Leaving Custom mode cancels any in-progress drag.
                                    if !mode.allows_drag() {
                                        self.dragging_char_id = None;
                                        self.drop_target_index = None;
                                    }
                                }
                            }
                        }
                    });
                sort_resp.response.hover_tip(self.sort_mode.tooltip());

                ui.separator();

                ui.label(RichText::new("Stats:").small().color(Color32::GRAY));
                egui::ComboBox::from_id_salt("characters_stat_display")
                    .selected_text(self.stat_display.label())
                    .show_ui(ui, |ui| {
                        for mode in [
                            StatDisplay::None,
                            StatDisplay::Current,
                            StatDisplay::LeftToMax,
                            StatDisplay::CurrentLeftToMax,
                        ] {
                            if ui
                                .selectable_label(self.stat_display == mode, mode.label())
                                .clicked()
                                && self.stat_display != mode
                            {
                                self.stat_display = mode;
                                settings_changed = true;
                            }
                        }
                    });

                ui.separator();

                ui.label(RichText::new("Show:").small().color(Color32::GRAY));
                if shadcn
                    .tgl(ui, &mut self.show_inventory, "Inventory")
                    .changed()
                {
                    settings_changed = true;
                }
                if shadcn
                    .tgl(ui, &mut self.show_backpack, "Backpack")
                    .changed()
                {
                    settings_changed = true;
                }
                if shadcn
                    .tgl(ui, &mut self.show_extender, "Extender")
                    .changed()
                {
                    settings_changed = true;
                }
                if shadcn.tgl(ui, &mut self.show_belt, "Belt").changed() {
                    settings_changed = true;
                }

                ui.separator();
                // Search box (class / ID / label) with autocomplete.
                self.render_search_box(ui, shadcn, sprite_renderer);
            });
        });

        if settings_changed {
            self.pending_edits
                .push(AppAction::SaveCharactersSettings(self.to_settings()));
        }

        // If the active tab has no search matches, switch to one that does.
        self.auto_switch_search_tab();

        // Header bar with tabs on left, controls on right
        shadcn.header_band(ui, shadcn.secondary_header_fill(), |ui| {
            shadcn.band_row(ui, |ui| {
                // Tab buttons with icons - Seasonal first
                let seasonal_label = format!("Seasonal ({})", self.cached_seasonal_count);
                let seasonal_icon = Some(get_character_subtab_icon(true));
                if sprite_renderer
                    .render_icon_button(
                        ui,
                        seasonal_icon,
                        &seasonal_label,
                        self.active_tab == CharacterTab::Seasonal,
                        Some(Color32::WHITE),
                        None,
                    )
                    .clicked()
                {
                    self.active_tab = CharacterTab::Seasonal;
                }

                let regular_label = format!("Regular ({})", self.cached_regular_count);
                let regular_icon = Some(get_character_subtab_icon(false));
                if sprite_renderer
                    .render_icon_button(
                        ui,
                        regular_icon,
                        &regular_label,
                        self.active_tab == CharacterTab::Regular,
                        Some(Color32::WHITE),
                        None,
                    )
                    .clicked()
                {
                    self.active_tab = CharacterTab::Regular;
                }

                let graveyard_label = format!("Graveyard ({})", self.cached_dead_count);
                let graveyard_icon = Some(get_graveyard_subtab_icon());
                if sprite_renderer
                    .render_icon_button(
                        ui,
                        graveyard_icon,
                        &graveyard_label,
                        self.active_tab == CharacterTab::Graveyard,
                        Some(Color32::WHITE),
                        None,
                    )
                    .clicked()
                {
                    self.active_tab = CharacterTab::Graveyard;
                }
            });
        });

        // Character grid. The Graveyard tab always uses the grid (it has its own
        // empty message); other tabs show the generic empty state only when no
        // character data exists at all.
        if self.active_tab == CharacterTab::Graveyard {
            self.render_character_grid(ui, sprite_renderer, shadcn);
        } else if self.cache.characters.is_empty() {
            self.render_empty_state(ui, access_token);
        } else {
            self.render_character_grid(ui, sprite_renderer, shadcn);
        }

        // Render label edit modal if open
        self.render_label_edit_modal(ui, shadcn);

        // Return any pending action (like viewing loot)
        self.pending_action.take()
    }

    /// Render empty state when no characters are cached.
    fn render_empty_state(&mut self, ui: &mut egui::Ui, access_token: &Option<String>) {
        ui.vertical_centered(|ui| {
            ui.add_space(50.0);
            ui.label(
                RichText::new("📦 No character data")
                    .size(20.0)
                    .color(Color32::GRAY),
            );
            ui.add_space(10.0);

            if access_token.is_some() {
                ui.label("Click 'Refresh' to fetch your characters from the API.");
                ui.label(
                    RichText::new("(Game must be closed to use the API)")
                        .small()
                        .color(Color32::GRAY),
                );
            } else {
                ui.label("To populate character data:");
                ui.add_space(5.0);
                ui.label("1. Click 'Start Capture' in the left sidebar");
                ui.label("2. Log into RotMG (this captures your access token)");
                ui.label("3. Play a character OR close the game and click 'Refresh'");
            }
        });
    }

    /// Sort a character list in place for the given [`CharacterSort`] mode.
    /// `Custom` preserves the incoming (drag) order.
    fn sort_characters(chars: &mut [CachedCharacter], mode: CharacterSort) {
        match mode {
            CharacterSort::Custom => {}
            CharacterSort::FameDesc => chars.sort_by(|a, b| b.fame.cmp(&a.fame)),
            CharacterSort::FameAsc => chars.sort_by(|a, b| a.fame.cmp(&b.fame)),
            CharacterSort::MaxedDesc => {
                chars.sort_by(|a, b| b.maxed_count.cmp(&a.maxed_count).then(b.fame.cmp(&a.fame)))
            }
            CharacterSort::MaxedAsc => {
                chars.sort_by(|a, b| a.maxed_count.cmp(&b.maxed_count).then(a.fame.cmp(&b.fame)))
            }
            CharacterSort::Class => chars.sort_by(|a, b| {
                a.class_name()
                    .cmp(b.class_name())
                    .then(a.char_id.cmp(&b.char_id))
            }),
            CharacterSort::IdAsc => chars.sort_by(|a, b| a.char_id.cmp(&b.char_id)),
            CharacterSort::IdDesc => chars.sort_by(|a, b| b.char_id.cmp(&a.char_id)),
        }
    }

    /// Whether a character matches the search query, testing class name, ID and
    /// label (case-insensitive substring). An empty query matches everything.
    fn char_matches_search(&self, char: &CachedCharacter, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        if char.class_name().to_lowercase().contains(&q) {
            return true;
        }
        // IDs are shown prefixed with '#', so ignore a leading '#' when matching.
        let id_q = q.trim_start_matches('#');
        if !id_q.is_empty() && char.char_id.to_string().contains(id_q) {
            return true;
        }
        self.cache
            .get_label(char.char_id)
            .map(|l| l.to_lowercase().contains(&q))
            .unwrap_or(false)
    }

    /// Number of characters in a given tab that match the search query.
    fn tab_match_count(&self, tab: CharacterTab, query: &str) -> usize {
        match tab {
            CharacterTab::Graveyard => self
                .cache
                .get_dead_characters()
                .iter()
                .filter(|c| self.char_matches_search(c, query))
                .count(),
            CharacterTab::Seasonal => self
                .cache
                .get_sorted_characters(true)
                .iter()
                .filter(|c| self.char_matches_search(c, query))
                .count(),
            CharacterTab::Regular => self
                .cache
                .get_sorted_characters(false)
                .iter()
                .filter(|c| self.char_matches_search(c, query))
                .count(),
        }
    }

    /// If the active tab has no search matches but another tab does, switch to
    /// the first tab (Regular, Seasonal, then Graveyard) that has matches.
    fn auto_switch_search_tab(&mut self) {
        let query = self.search_query.trim().to_string();
        if query.is_empty() || self.tab_match_count(self.active_tab, &query) > 0 {
            return;
        }
        for tab in [
            CharacterTab::Regular,
            CharacterTab::Seasonal,
            CharacterTab::Graveyard,
        ] {
            if tab != self.active_tab && self.tab_match_count(tab, &query) > 0 {
                self.active_tab = tab;
                return;
            }
        }
    }

    /// Build up to 8 de-duplicated autocomplete suggestions (class names, IDs and
    /// labels) matching the current query across all cached characters.
    fn search_suggestions(&self, query: &str) -> Vec<SearchSuggestion> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<SearchSuggestion> = Vec::new();
        let seen = |out: &[SearchSuggestion], s: &str| {
            out.iter().any(|e| e.text().eq_ignore_ascii_case(s))
        };
        let all = self
            .cache
            .characters
            .iter()
            .chain(self.cache.dead_characters.iter());
        for char in all {
            if out.len() >= 8 {
                break;
            }
            let skin_id = if char.skin > 0 {
                char.skin
            } else {
                char.class_id as i32
            };
            let class = char.class_name();
            if class.to_lowercase().contains(&q) && !seen(&out, class) {
                out.push(SearchSuggestion::Class {
                    name: class.to_string(),
                    class_id: char.class_id as i32,
                });
            }
            let id = char.char_id.to_string();
            let id_q = q.trim_start_matches('#');
            let id_display = format!("#{}", id);
            if !id_q.is_empty() && id.contains(id_q) && !seen(&out, &id_display) {
                out.push(SearchSuggestion::Character {
                    text: id_display,
                    skin_id,
                    tex1: char.tex1,
                    tex2: char.tex2,
                });
            }
            if let Some(label) = self.cache.get_label(char.char_id) {
                if label.to_lowercase().contains(&q) && !seen(&out, label) {
                    out.push(SearchSuggestion::Character {
                        text: label.to_string(),
                        skin_id,
                        tex1: char.tex1,
                        tex2: char.tex2,
                    });
                }
            }
        }
        out.truncate(8);
        out
    }

    /// Render the toolbar search box with class/ID/label autocomplete.
    fn render_search_box(
        &mut self,
        ui: &mut egui::Ui,
        shadcn: &Shadcn,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        let resp = ui.add(
            egui::TextEdit::singleline(&mut self.search_query)
                .id_source("characters_search_box")
                .desired_width(220.0)
                .hint_text("Search by class, ID or label..."),
        );
        if resp.has_focus() && !self.search_query.trim().is_empty() {
            self.search_popup_open = true;
        }
        if resp.changed() {
            self.search_selected_index = None;
        }
        let enter_pressed = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

        if !self.search_query.is_empty() && shadcn.btn(ui, "✕").clicked() {
            self.search_query.clear();
            self.search_popup_open = false;
            self.search_selected_index = None;
        }

        let suggestions = self.search_suggestions(&self.search_query);
        let total = suggestions.len();

        // Enter applies the highlighted (or first) suggestion.
        if enter_pressed && self.search_popup_open && total > 0 {
            let idx = self.search_selected_index.unwrap_or(0);
            if let Some(s) = suggestions.get(idx) {
                self.search_query = s.text().to_string();
                self.search_popup_open = false;
                self.search_selected_index = None;
            }
        }

        // Arrow-key navigation / Escape while the field is focused.
        if self.search_popup_open && resp.has_focus() && total > 0 {
            let key_down = ui.input(|i| i.key_pressed(egui::Key::ArrowDown));
            let key_up = ui.input(|i| i.key_pressed(egui::Key::ArrowUp));
            let key_escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
            if key_escape {
                self.search_popup_open = false;
                self.search_selected_index = None;
            } else if key_down {
                self.search_selected_index = Some(match self.search_selected_index {
                    None => 0,
                    Some(i) => (i + 1).min(total - 1),
                });
            } else if key_up {
                self.search_selected_index = match self.search_selected_index {
                    None => None,
                    Some(0) => None,
                    Some(i) => Some(i - 1),
                };
            }
        }

        // Suggestion popup anchored under the text box.
        if self.search_popup_open && total > 0 {
            let popup_id = ui.make_persistent_id("characters_search_popup");
            let popup_pos = resp.rect.left_bottom() + egui::vec2(0.0, 2.0);
            let mut chosen: Option<String> = None;
            egui::Area::new(popup_id)
                .fixed_pos(popup_pos)
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_min_width(220.0);
                        ui.set_max_height(300.0);
                        egui::ScrollArea::vertical()
                            .max_height(280.0)
                            .show(ui, |ui| {
                                for (idx, s) in suggestions.iter().enumerate() {
                                    let is_selected = self.search_selected_index == Some(idx);
                                    let mut clicked = false;
                                    ui.horizontal(|ui| {
                                        // Class suggestions show the default class sprite;
                                        // ID/label suggestions show the character's dyed
                                        // skin so labels map to their character.
                                        let (rect, _) = ui.allocate_exact_size(
                                            egui::vec2(20.0, 20.0),
                                            egui::Sense::hover(),
                                        );
                                        let (icon_id, tex1, tex2) = match s {
                                            SearchSuggestion::Class { class_id, .. } => {
                                                (*class_id, 0, 0)
                                            }
                                            SearchSuggestion::Character {
                                                skin_id,
                                                tex1,
                                                tex2,
                                                ..
                                            } => (*skin_id, *tex1, *tex2),
                                        };
                                        if icon_id > 0 {
                                            sprite_renderer.draw_dyed_outlined_character_sprite(
                                                ui, icon_id, rect, 6, tex1, tex2,
                                            );
                                        }
                                        let r = ui.add(
                                            egui::Button::new(s.text())
                                                .selected(is_selected)
                                                .min_size(egui::vec2(186.0, 0.0)),
                                        );
                                        if is_selected {
                                            r.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        if r.clicked() {
                                            clicked = true;
                                        }
                                    });
                                    if clicked {
                                        chosen = Some(s.text().to_string());
                                    }
                                }
                            });
                    });
                });
            if let Some(s) = chosen {
                self.search_query = s;
                self.search_popup_open = false;
                self.search_selected_index = None;
            }
            // Close the popup when clicking outside the text field.
            if ui.input(|i| i.pointer.any_click()) && !resp.has_focus() {
                self.search_popup_open = false;
                self.search_selected_index = None;
            }
        }
    }

    /// Render the character grid with responsive columns using horizontal_wrapped.
    fn render_character_grid(
        &mut self,
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        shadcn: &Shadcn,
    ) {
        let is_graveyard = self.active_tab == CharacterTab::Graveyard;

        // Get characters for the active tab.
        let mut characters: Vec<CachedCharacter> = if is_graveyard {
            let mut chars: Vec<CachedCharacter> =
                self.cache.get_dead_characters().iter().cloned().collect();
            Self::sort_characters(&mut chars, self.sort_mode);
            chars
        } else {
            let show_seasonal = matches!(self.active_tab, CharacterTab::Seasonal);
            // Custom mode uses the persisted drag order; every other mode sorts
            // a fresh copy of the tab's characters.
            let mut chars: Vec<CachedCharacter> = self
                .cache
                .get_sorted_characters(show_seasonal)
                .iter()
                .map(|c| (*c).clone())
                .collect();
            Self::sort_characters(&mut chars, self.sort_mode);
            chars
        };
        // Apply the toolbar search filter (class / ID / label).
        if !self.search_query.trim().is_empty() {
            let query = self.search_query.clone();
            characters.retain(|c| self.char_matches_search(c, &query));
        }
        let show_seasonal = matches!(self.active_tab, CharacterTab::Seasonal);

        if characters.is_empty() {
            if !self.search_query.trim().is_empty() {
                crate::panels::empty_state(
                    ui,
                    &format!("No characters match \"{}\"", self.search_query.trim()),
                    None,
                );
                return;
            }
            let tab_name = match self.active_tab {
                CharacterTab::Seasonal => "seasonal",
                CharacterTab::Regular => "regular",
                CharacterTab::Graveyard => "dead",
            };
            crate::panels::empty_state(ui, &format!("No {} characters", tab_name), None);
            return;
        }

        // Drag-and-drop reordering is disabled in the Graveyard tab and in any
        // sort mode other than Custom.
        let drag_enabled = !is_graveyard && self.sort_mode.allows_drag();
        let is_dragging = drag_enabled && self.dragging_char_id.is_some();
        let mut card_rects: Vec<(i32, egui::Rect)> = if is_dragging {
            Vec::with_capacity(characters.len())
        } else {
            Vec::new()
        };

        // Compute drop target before rendering (for visual indicator)
        let current_drop_target = if let Some(dragging_id) = self.dragging_char_id {
            if let Some(pointer_pos) = ui.input(|i| i.pointer.interact_pos()) {
                // We'll compute it properly after we have card_rects, store pointer for now
                Some((dragging_id, pointer_pos))
            } else {
                None
            }
        } else {
            None
        };

        // While dragging a card, egui disables the ScrollArea's own wheel
        // handling (it gates wheel scroll behind `dragged_id().is_none()`), and
        // its drag-to-scroll would auto-pan the list from the drag gesture. We
        // disable drag-to-scroll to stop the auto-pan and re-apply wheel input
        // manually below so the user can still scroll into off-screen rows mid
        // drag.
        let scroll_source = if is_dragging {
            egui::containers::scroll_area::ScrollSource {
                drag: false,
                ..egui::containers::scroll_area::ScrollSource::ALL
            }
        } else {
            egui::containers::scroll_area::ScrollSource::ALL
        };

        let nav = super::scroll_nav::ScrollNav::read(ui, "characters_list_scroll");
        let mut scroll_area = ScrollArea::vertical()
            .auto_shrink([false, false])
            .scroll_source(scroll_source);
        scroll_area = nav.apply(scroll_area);
        let scroll_out = scroll_area.show(ui, |ui| {
            // egui suppresses wheel scrolling while any widget is being
            // dragged, so drive it manually from the raw wheel delta.
            if is_dragging {
                let wheel = ui.input(|i| i.raw_scroll_delta.y);
                if wheel != 0.0 {
                    ui.scroll_with_delta(egui::vec2(0.0, wheel));
                }
            }
            // Use horizontal_wrapped for automatic line-breaking of cards.
            // This respects the available width and wraps cards to the next row.
            ui.horizontal_wrapped(|ui| {
                // Set spacing between cards
                ui.spacing_mut().item_spacing = egui::vec2(CARD_SPACING, CARD_SPACING);
                // Align cards to top so varying heights don't misalign
                ui.style_mut().spacing.item_spacing.y = CARD_SPACING;

                for (index, char) in characters.iter().enumerate() {
                    let card_rect = self.render_character_card_draggable(
                        ui,
                        char,
                        index,
                        is_graveyard,
                        sprite_renderer,
                        shadcn,
                    );
                    // Only track rects when dragging to avoid per-frame allocations
                    if is_dragging {
                        card_rects.push((char.char_id, card_rect));
                    }
                }
            });

            // Draw drop indicator line if dragging
            if let Some((dragging_id, pointer_pos)) = current_drop_target {
                // Find the dragged card's rect to check if pointer is over it
                let dragged_card_rect = card_rects
                    .iter()
                    .find(|(id, _)| *id == dragging_id)
                    .map(|(_, rect)| rect);

                // Only show indicator if pointer is NOT over the dragged card itself
                let over_dragged_card = dragged_card_rect
                    .map(|rect| rect.contains(pointer_pos))
                    .unwrap_or(false);

                if over_dragged_card {
                    // Hovering over the card we're dragging - no indicator, no move
                    self.drop_target_index = None;
                } else {
                    let drop_index = self.compute_drop_index(&card_rects, pointer_pos, dragging_id);
                    self.drop_target_index = Some(drop_index);

                    // Find the position for the indicator line
                    if let Some(indicator_pos) =
                        self.get_drop_indicator_position(&card_rects, drop_index, dragging_id)
                    {
                        let painter = ui.painter();
                        let indicator_color = Color32::from_rgb(255, 200, 50);

                        // Draw a vertical line with rounded ends
                        painter.line_segment(
                            [indicator_pos.0, indicator_pos.1],
                            egui::Stroke::new(3.0_f32, indicator_color),
                        );

                        // Draw small triangles/arrows at top and bottom pointing to the line
                        let arrow_size = 6.0;
                        // Top arrow pointing down
                        painter.add(egui::Shape::convex_polygon(
                            vec![
                                egui::pos2(
                                    indicator_pos.0.x - arrow_size,
                                    indicator_pos.0.y - arrow_size,
                                ),
                                egui::pos2(
                                    indicator_pos.0.x + arrow_size,
                                    indicator_pos.0.y - arrow_size,
                                ),
                                egui::pos2(indicator_pos.0.x, indicator_pos.0.y),
                            ],
                            indicator_color,
                            egui::Stroke::NONE,
                        ));
                        // Bottom arrow pointing up
                        painter.add(egui::Shape::convex_polygon(
                            vec![
                                egui::pos2(
                                    indicator_pos.1.x - arrow_size,
                                    indicator_pos.1.y + arrow_size,
                                ),
                                egui::pos2(
                                    indicator_pos.1.x + arrow_size,
                                    indicator_pos.1.y + arrow_size,
                                ),
                                egui::pos2(indicator_pos.1.x, indicator_pos.1.y),
                            ],
                            indicator_color,
                            egui::Stroke::NONE,
                        ));
                    }
                }
            } else {
                self.drop_target_index = None;
            }
        });
        nav.store(ui, &scroll_out);

        // Handle drag release
        if let Some(dragging_id) = self.dragging_char_id {
            if ui.input(|i| i.pointer.any_released()) {
                // Use the already computed drop target
                if let Some(target_index) = self.drop_target_index {
                    self.cache
                        .move_character(dragging_id, target_index, show_seasonal);
                    self.mark_needs_save();
                    self.pending_edits.push(AppAction::MoveCharacter {
                        char_id: dragging_id,
                        to_index: target_index,
                        seasonal: show_seasonal,
                    });
                }
                self.dragging_char_id = None;
                self.drop_target_index = None;
            }
        }
    }

    /// Get the position for the drop indicator line (top and bottom points).
    fn get_drop_indicator_position(
        &self,
        card_rects: &[(i32, egui::Rect)],
        drop_index: usize,
        dragging_id: i32,
    ) -> Option<(egui::Pos2, egui::Pos2)> {
        if card_rects.is_empty() {
            return None;
        }

        // Filter out the dragging card to get target positions
        let non_dragging: Vec<(usize, &egui::Rect)> = card_rects
            .iter()
            .enumerate()
            .filter(|(_, (id, _))| *id != dragging_id)
            .map(|(i, (_, rect))| (i, rect))
            .collect();

        if non_dragging.is_empty() {
            return None;
        }

        // Determine which rect to use for the indicator
        let (x, y_top, y_bottom) = if drop_index == 0 {
            // Dropping at the beginning - draw line at left of first non-dragging card
            let rect = non_dragging.first()?.1;
            (rect.left() - CARD_SPACING / 2.0, rect.top(), rect.bottom())
        } else if drop_index >= card_rects.len() {
            // Dropping at the end - draw line at right of last card
            let rect = non_dragging.last()?.1;
            (rect.right() + CARD_SPACING / 2.0, rect.top(), rect.bottom())
        } else {
            // Dropping in the middle - draw line between cards
            // Find the card at drop_index (the one we're inserting before)
            if let Some((_, rect)) = card_rects.get(drop_index) {
                (rect.left() - CARD_SPACING / 2.0, rect.top(), rect.bottom())
            } else {
                return None;
            }
        };

        Some((egui::pos2(x, y_top), egui::pos2(x, y_bottom)))
    }

    /// Compute the drop index based on pointer position relative to card rects.
    fn compute_drop_index(
        &self,
        card_rects: &[(i32, egui::Rect)],
        pointer_pos: egui::Pos2,
        _dragging_id: i32,
    ) -> usize {
        // Find which card the pointer is over
        for (index, (_char_id, rect)) in card_rects.iter().enumerate() {
            // Check if pointer is in the left half of this card (insert before) or right half (insert after)
            if pointer_pos.y >= rect.min.y && pointer_pos.y <= rect.max.y {
                if pointer_pos.x < rect.center().x {
                    return index;
                } else if pointer_pos.x <= rect.max.x {
                    return index + 1;
                }
            }
        }
        // Default to end
        card_rects.len()
    }

    /// Render a single character card with drag support. Returns the card's rect.
    ///
    /// In graveyard mode the card uses normal (non-dead) frame styling, renders
    /// the matching grave sprite, suppresses the DEAD badge, and disables drag
    /// reordering (the Remove Dead Character action stays available).
    fn render_character_card_draggable(
        &mut self,
        ui: &mut egui::Ui,
        char: &CachedCharacter,
        index: usize,
        is_graveyard: bool,
        sprite_renderer: &mut SpriteRenderer,
        shadcn: &Shadcn,
    ) -> egui::Rect {
        let is_live = self.live_char_id == Some(char.char_id);
        // In the Graveyard tab every card is dead, so suppress the red dead styling.
        let is_dead = char.is_dead && !is_graveyard;
        let is_being_dragged = self.dragging_char_id == Some(char.char_id);
        let is_drop_target = self.drop_target_index == Some(index);
        // Clone the label to avoid borrow conflict
        let custom_label = self
            .cache
            .get_label(char.char_id)
            .map(|s| truncate_label(s.to_string()));
        // Look up the equipped pet (name + sprite) for the header line.
        // Use the sprite/object name (as the Vault Pet Inventories section does)
        // rather than the raw API pet name, which can be wrong.
        let pet_info: Option<(String, i32)> = char.pet_instance_id.and_then(|id| {
            self.cache
                .resolve_pet_identity(id, char.seasonal)
                .map(|pet| {
                    let sprite = if pet.skin > 0 { pet.skin } else { pet.pet_type };
                    let name = realmhound_core::assets::get_asset_manager()
                        .object_name(sprite)
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| pet.name.clone());
                    (name, sprite)
                })
        });
        let stat_display = self.stat_display;
        let (show_inv, show_bp, show_ext, show_belt) = (
            self.show_inventory,
            self.show_backpack,
            self.show_extender,
            self.show_belt,
        );
        let class_name = char.class_name().to_string();
        let char_id = char.char_id;
        // Capture icon_id for loot filter (prefer skin, fall back to class ID)
        let icon_id = if char.skin > 0 {
            char.skin
        } else {
            char.class_id as i32
        };

        // Card frame - dead characters get a dark red tint
        let card_fill = crate::ui_colors::card_fill(ui.visuals());
        let (mut fill, mut stroke_color) = if is_dead {
            (
                Color32::from_rgb(40, 20, 20),
                Color32::from_rgb(150, 50, 50),
            )
        } else if is_live {
            (card_fill, Color32::from_rgb(100, 255, 100))
        } else {
            (card_fill, crate::ui_colors::card_stroke(ui.visuals()))
        };

        // Visual feedback for drag/drop
        if is_being_dragged {
            fill = Color32::from_rgba_unmultiplied(fill.r(), fill.g(), fill.b(), 150);
            stroke_color = Color32::from_rgb(100, 150, 255);
        } else if is_drop_target {
            stroke_color = Color32::from_rgb(255, 200, 50);
        }

        let stroke_width = if is_live || is_dead || is_being_dragged || is_drop_target {
            2.0_f32
        } else {
            1.0_f32
        };

        // Card width = content + padding*2 + stroke*2
        let card_width = CARD_CONTENT_WIDTH + CARD_PADDING * 2.0 + 4.0;

        // Use allocate_ui to reserve fixed width while allowing dynamic height.
        // This ensures wrapping works correctly while tall cards don't overlap.
        let frame_response = ui.allocate_ui(egui::vec2(card_width, ui.available_height()), |ui| {
            ui.push_id(char_id, |ui| {
                egui::Frame::NONE
                    .fill(fill)
                    .stroke(egui::Stroke::new(stroke_width, stroke_color))
                    .corner_radius(8.0)
                    .inner_margin(CARD_PADDING)
                    .show(ui, |ui| {
                        // Constrain content width
                        ui.set_min_width(CARD_CONTENT_WIDTH);
                        ui.set_max_width(CARD_CONTENT_WIDTH);

                        // Delegate card content to shared CharacterCardWidget (full mode)
                        ui.vertical(|ui| {
                            let widget = if is_graveyard {
                                let grave = grave_sprite_for_char(
                                    char.gravestone_type,
                                    char.maxed_count,
                                    char.level,
                                );
                                CharacterCardWidget::full_graveyard(grave)
                            } else {
                                CharacterCardWidget::full()
                            };
                            let widget = widget
                                .with_stat_display(stat_display)
                                .with_pet(pet_info.clone())
                                .with_loot_boost(
                                    self.loot_boost_by_char.get(&char.char_id).copied(),
                                )
                                .with_sections(show_inv, show_bp, show_ext, show_belt);
                            widget.render(
                                ui,
                                char,
                                is_live,
                                custom_label.as_deref(),
                                sprite_renderer,
                            );
                        });
                    })
            })
        });

        let card_rect = frame_response.response.rect;

        // Handle drag interaction manually (left-click only)
        // Check if primary button is down and we're over this card
        // Don't use ui.interact() as it would block hover events for inner item tooltips
        let pointer_over_card = ui.rect_contains_pointer(card_rect);
        // Dragging only applies in Custom sort mode outside the Graveyard tab.
        let drag_enabled = !is_graveyard && self.sort_mode.allows_drag();
        // Show a move cursor on hover to signal the card can be dragged.
        if drag_enabled && pointer_over_card {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Move);
        }
        if drag_enabled && pointer_over_card && ui.input(|i| i.pointer.primary_down()) {
            // Start drag if not already dragging and mouse has moved
            if self.dragging_char_id.is_none() {
                let drag_delta = ui.input(|i| i.pointer.delta());
                if drag_delta.length_sq() > 0.0 {
                    self.dragging_char_id = Some(char_id);
                }
            }
        }

        // Right-click context menu - show at click position
        let right_clicked = pointer_over_card && ui.input(|i| i.pointer.secondary_clicked());

        if right_clicked {
            self.context_menu_pos = ui.input(|i| i.pointer.hover_pos());
            self.context_menu_char_id = Some(char_id);
        }

        // Show popup if this card's menu is open
        if self.context_menu_char_id == Some(char_id) {
            let menu_pos = self.context_menu_pos.unwrap_or(card_rect.center());
            let popup_id = ui.id().with(("char_context_menu", char_id));

            let area_resp = egui::Area::new(popup_id)
                .order(egui::Order::Foreground)
                .fixed_pos(menu_pos)
                .interactable(true)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        // Constrain popup to fixed width to prevent stretching
                        ui.allocate_ui_with_layout(
                            egui::vec2(180.0, 0.0),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                if shadcn.btn(ui, "📊 View Stats").clicked() {
                                    self.viewing_stats_for = Some(char_id);
                                    self.stats_modal_opened_frame = ui.ctx().cumulative_frame_nr();
                                    self.context_menu_char_id = None;
                                }
                                if shadcn.btn(ui, "📦 View Loot").clicked() {
                                    let char_name =
                                        custom_label.clone().unwrap_or(class_name.clone());
                                    self.pending_action = Some(CharacterAction::ViewLoot(
                                        char_id, char_name, icon_id,
                                    ));
                                    self.context_menu_char_id = None;
                                }
                                if shadcn.btn(ui, "⚔ View boss fights").clicked() {
                                    let char_name =
                                        custom_label.clone().unwrap_or(class_name.clone());
                                    self.pending_action =
                                        Some(CharacterAction::ViewFights(char_id, char_name));
                                    self.context_menu_char_id = None;
                                }
                                if shadcn.btn(ui, "✏ Edit Label").clicked() {
                                    self.editing_label = Some(char_id);
                                    self.label_dialog_open = true;
                                    self.label_dialog_focus = true;
                                    self.label_edit_buffer =
                                        custom_label.clone().unwrap_or(class_name.clone());
                                    self.context_menu_char_id = None;
                                }
                                if char.is_dead {
                                    ui.separator();
                                    if shadcn.btn(ui, "💀 Remove Dead Character").clicked() {
                                        self.remove_character(char_id);
                                        self.context_menu_char_id = None;
                                    }
                                }
                            },
                        );
                    });
                });

            // Close when clicking outside the popup area
            let popup_rect = area_resp.response.rect;
            if ui.input(|i| i.pointer.any_pressed()) && !right_clicked {
                let pointer_pos = ui.input(|i| i.pointer.hover_pos());
                if let Some(pos) = pointer_pos {
                    if !popup_rect.contains(pos) {
                        self.context_menu_char_id = None;
                    }
                }
            }
        }

        card_rect
    }

    /// Render the full-panel stats view when viewing_stats_for is set.
    fn render_stats_view(
        &mut self,
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        shadcn: &Shadcn,
    ) {
        let Some(char_id) = self.viewing_stats_for else {
            return;
        };

        // Find the character (living or dead, so graveyard chars resolve too)
        let char_data = self.cache.find_any_character(char_id).cloned();
        let Some(char) = char_data else {
            self.viewing_stats_for = None;
            return;
        };

        // Get custom label
        let custom_label = self
            .cache
            .get_label(char_id)
            .map(|s| truncate_label(s.to_string()));

        // Decode stats
        let stats = decode_pcstats(&char.pc_stats_raw);

        // Snapshot the persisted stats-layout state so we can detect user edits
        // (drag/drop, category toggles, dungeon sort, breakdown) this frame and
        // flush them to disk once at the end.
        let layout_before = (
            self.stats_block_order,
            self.stats_block_column,
            self.category_visible,
            self.dungeon_sort,
            self.stats_show_breakdown,
        );

        // Narrow navigation-only bar (like the Combat History fight card):
        // back-to-sub-tab, Previous/Next within the sub-tab, dungeon order, and
        // per-category visibility.
        let tab_ids = self.tab_character_ids();
        let cur_idx = tab_ids.iter().position(|&id| id == char_id);
        let has_prev = cur_idx.is_some_and(|i| i > 0);
        let has_next = cur_idx.is_some_and(|i| i + 1 < tab_ids.len());
        let back_label = format!("← Back to {}", self.tab_display_name());

        let mut nav_back = false;
        let mut nav_prev = false;
        let mut nav_next = false;
        shadcn.header_band(ui, shadcn.secondary_header_fill(), |ui| {
            shadcn.band_row(ui, |ui| {
                if shadcn.btn(ui, back_label).clicked() {
                    nav_back = true;
                }
                if ui
                    .add_enabled(has_prev, egui::Button::new("<< Previous"))
                    .clicked()
                {
                    nav_prev = true;
                }
                if ui
                    .add_enabled(has_next, egui::Button::new("Next >>"))
                    .clicked()
                {
                    nav_next = true;
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(10.0);

                ui.label("Dungeon order:");
                egui::ComboBox::from_id_salt("character_dungeon_sort")
                    .selected_text(match self.dungeon_sort {
                        DungeonSort::Default => "Default",
                        DungeonSort::DifficultyAsc => "Difficulty ↑",
                        DungeonSort::DifficultyDesc => "Difficulty ↓",
                        DungeonSort::MostCompleted => "Most completed",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.dungeon_sort,
                            DungeonSort::Default,
                            "Default",
                        );
                        ui.selectable_value(
                            &mut self.dungeon_sort,
                            DungeonSort::DifficultyAsc,
                            "Difficulty ↑",
                        );
                        ui.selectable_value(
                            &mut self.dungeon_sort,
                            DungeonSort::DifficultyDesc,
                            "Difficulty ↓",
                        );
                        ui.selectable_value(
                            &mut self.dungeon_sort,
                            DungeonSort::MostCompleted,
                            "Most completed",
                        );
                    });

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(10.0);

                ui.menu_button("Show categories ▼", |ui| {
                    // Color the category toggles with the shared header color so
                    // the dropdown matches the on-page category headers.
                    ui.visuals_mut().override_text_color = Some(Color32::from_rgb(150, 200, 255));
                    for i in 0..CAT_COUNT {
                        ui.checkbox(&mut self.category_visible[i], CATEGORY_LABELS[i]);
                    }
                });

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(10.0);

                ui.checkbox(&mut self.stats_show_breakdown, "Show stats breakdown")
                    .on_hover_text(
                        "Show the Gear / Exalts / Crucible columns in ATTRIBUTES. \
                         Off shows only the total stat values.",
                    );
            });
        });
        if nav_back {
            self.viewing_stats_for = None;
            return;
        }
        if let Some(i) = cur_idx {
            if nav_prev {
                self.viewing_stats_for = Some(tab_ids[i - 1]);
                self.stats_modal_opened_frame = ui.ctx().cumulative_frame_nr();
                return;
            }
            if nav_next {
                self.viewing_stats_for = Some(tab_ids[i + 1]);
                self.stats_modal_opened_frame = ui.ctx().cumulative_frame_nr();
                return;
            }
        }

        // Calculate available height for content (leave space for status bar).
        // The former footer (fame-on-death + export buttons) is now folded into
        // the ATTRIBUTES block and per-block copy buttons, so only a small
        // bottom margin is reserved.
        let footer_height = 16.0;
        let available_height = (ui.available_height() - footer_height).max(100.0);

        // Main content area with both horizontal and vertical scroll
        ui.add_space(8.0);

        // Read the dungeon sort once per frame so both dungeon columns render in
        // the same order even on the frame the user changes the combo.
        let dungeon_sort = self.dungeon_sort;
        // Whether the ATTRIBUTES card shows the Gear/Exalts/Crucible breakdown or
        // just the bright Total column (captured to avoid a borrow conflict with
        // the &mut self fields threaded into render_readout).
        let show_breakdown = self.stats_show_breakdown;

        // Theoretical stats are derived from equipped gear + enchants + pet and
        // don't depend on pc_stats, so compute once per frame regardless.
        let derived = derived_stats::compute(&char, &self.cache);
        // The stats page is an explicit view of a chosen character, so it always
        // renders fully (the dim + "log in" note is a widget-only stale state).
        let ctx = derived_stats::build_context(
            &char,
            &self.cache,
            self.loot_boost_by_char
                .get(&char.char_id)
                .copied()
                .unwrap_or(0)
                > 0,
            true,
        );

        // Character identity + fame lines folded into the ATTRIBUTES block
        // (skin/grave lead sprite, name/id/class/maxed line, base fame,
        // killed-by, and the fame-on-death total below Bonuses).
        let class_name = char.class_name();
        let id_title = format!(
            "{} Lv.{}",
            custom_label.as_deref().unwrap_or(class_name),
            char.level
        );
        let id_subtitle = format!(
            "#{} • {} • {}/8",
            char.char_id, class_name, char.maxed_count
        );
        let fame_on_death: Option<i64> = if char.is_dead {
            if char.death_fame > 0 {
                Some(char.death_fame as i64)
            } else {
                stats
                    .as_ref()
                    .map(|s| dead_fame_total(char.fame, s, char.maxed_flags(), DUNGEON_COLLECTIONS))
            }
        } else {
            stats
                .as_ref()
                .map(|s| dead_fame_total(char.fame, s, char.maxed_flags(), DUNGEON_COLLECTIONS))
        };
        let identity = derived_stats::StatsIdentity {
            skin_id: if char.skin > 0 {
                char.skin
            } else {
                char.class_id as i32
            },
            tex1: char.tex1,
            tex2: char.tex2,
            fallback_initial: class_name.chars().next().unwrap_or('?'),
            is_dead: char.is_dead,
            grave: if char.is_dead {
                Some(grave_sprite_for_char(
                    char.gravestone_type,
                    char.maxed_count,
                    char.level,
                ))
            } else {
                None
            },
            title: id_title,
            subtitle: id_subtitle,
            base_fame: char.fame,
            killed_by: if char.is_dead {
                Some(char.killed_by.as_deref().unwrap_or("Unknown").to_string())
            } else {
                None
            },
            killer_icon: if char.is_dead {
                realmhound_core::assets::get_asset_manager()
                    .killer_sprite_id(char.killed_by.as_deref().unwrap_or("Unknown"))
            } else {
                None
            },
            fame_on_death,
        };

        // Column count follows the window width: fit as many nominal-width
        // columns as the panel allows, always keeping at least one empty
        // trailing column past the last used one so a block can be dropped into
        // a fresh column. Blocks stay in their assigned column (no auto flow).
        let col_width = STATS_COL_WIDTH;
        let col_spacing = 4.0;
        let avail_w = ui.available_width();
        let fit_cols =
            (((avail_w + col_spacing) / (col_width + col_spacing)).floor() as usize).max(1);
        let used_cols = self
            .stats_block_order
            .iter()
            .copied()
            .filter(|&b| {
                self.category_visible[block_category(b)] && Self::stats_block_has_content(b, &char)
            })
            .map(|b| self.stats_block_column[b] + 1)
            .max()
            .unwrap_or(0);
        let displayed_cols = fit_cols.max(used_cols + 1);
        let mut new_widths = self.stats_block_widths;
        // Rendered card rects (block, rect) and per-column rects for drag/drop.
        let mut block_rects: Vec<(usize, egui::Rect)> = Vec::new();
        let mut col_rects: Vec<egui::Rect> = Vec::new();

        ScrollArea::both()
            .id_salt(("stats_view_scroll", self.stats_modal_opened_frame))
            .max_height(available_height)
            .auto_shrink([false, true])
            .scroll_source(egui::containers::scroll_area::ScrollSource {
                drag: false,
                ..egui::containers::scroll_area::ScrollSource::ALL
            })
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    if let Some(ref stats) = stats {
                        for c in 0..displayed_cols {
                            // Blocks assigned to this column, in order. Blocks
                            // with nothing to show (e.g. MISC STATS on a
                            // character with no such tallies) are omitted so no
                            // empty bordered card is drawn.
                            let col_blocks: Vec<usize> = self
                                .stats_block_order
                                .iter()
                                .copied()
                                .filter(|&b| {
                                    self.category_visible[block_category(b)]
                                        && self.stats_block_column[b] == c
                                        && Self::stats_block_has_content(b, &char)
                                })
                                .collect();
                            // Column width = its widest card's content width,
                            // floored at a small minimum (so a narrow-only column
                            // collapses toward its content) and clamped up top so
                            // a stray full-width widget can't run away. Empty
                            // (drop-target) columns use the full nominal width.
                            let target_w = if col_blocks.is_empty() {
                                col_width
                            } else {
                                col_blocks
                                    .iter()
                                    .map(|&b| self.stats_block_widths[b])
                                    .fold(STATS_MIN_CARD_W, f32::max)
                                    .min(700.0)
                            };
                            ui.add_space(col_spacing);
                            let col_resp = ui.vertical(|ui| {
                                ui.set_min_width(target_w);
                                // Reserve full height so an empty column is still
                                // a drop target across the viewport.
                                ui.set_min_height(available_height);
                                for (bi, &block) in col_blocks.iter().enumerate() {
                                    if bi > 0 {
                                        ui.add_space(8.0);
                                    }
                                    // Each category block sits in its own rounded
                                    // "card" (thin grey outline like the character
                                    // cards) instead of being split by vertical
                                    // column separators.
                                    let inner = egui::Frame::NONE
                                        .stroke(egui::Stroke::new(
                                            1.0_f32,
                                            crate::ui_colors::card_stroke(ui.visuals()),
                                        ))
                                        .corner_radius(8.0)
                                        .inner_margin(6.0)
                                        .show(ui, |ui| {
                                            // Bound available_width so full-width
                                            // separators can't expand (and diverge)
                                            // in the non-shrinking scroll area, but
                                            // do NOT force a minimum yet so narrow
                                            // content can be measured at its natural
                                            // width (and the column can shrink).
                                            ui.set_max_width(target_w);
                                            match block {
                                                STATS_BLOCK_ATTRIBUTES => {
                                                    derived_stats::render_readout(ui, sprite_renderer, &derived, &mut self.theoretical_target_def, &mut self.target_def_text, &char.maxed_flags(), &ctx, true, true, show_breakdown, Some(&identity));
                                                }
                                                STATS_BLOCK_MISC => {
                                                    Self::render_misc_stats(ui, sprite_renderer, &char);
                                                }
                                                STATS_BLOCK_COLLECTIONS => {
                                                    self.render_collections_column(ui, stats, sprite_renderer, char.fame);
                                                }
                                                STATS_BLOCK_DUNGEONS_0 => {
                                                    self.render_dungeons_column(ui, stats, sprite_renderer, 0, dungeon_sort);
                                                }
                                                STATS_BLOCK_DUNGEONS_1 => {
                                                    self.render_dungeons_column(ui, stats, sprite_renderer, 1, dungeon_sort);
                                                }
                                                STATS_BLOCK_BIOMES => {
                                                    self.render_biomes_column(ui, stats, sprite_renderer, None);
                                                }
                                                STATS_BLOCK_STATISTICS => {
                                                    self.render_stats_column(ui, stats, sprite_renderer);
                                                }
                                                _ => {}
                                            }
                                            // Natural content width (excludes the
                                            // frame margin/stroke) measured BEFORE
                                            // forcing the shared column width, so
                                            // narrow cards shrink instead of
                                            // sticking at the column width.
                                            let natural = ui.min_rect().width();
                                            // Now expand to the shared column width
                                            // so every card in the column (and its
                                            // stroke) lines up to the same edge.
                                            ui.set_min_width(target_w);
                                            natural
                                        });
                                    let card_rect = inner.response.rect;
                                    let content_w = inner.inner;
                                    if block < new_widths.len() {
                                        new_widths[block] = content_w;
                                    }
                                    // Edge-drag handles: interact only on the
                                    // thin outer margin strips (the frame's
                                    // inner_margin) so inner widgets (headers,
                                    // text fields) keep their own clicks.
                                    let e = 8.0;
                                    let id = ui.make_persistent_id(("stats_card_edge", block));
                                    let top = egui::Rect::from_min_max(card_rect.min, egui::pos2(card_rect.max.x, card_rect.min.y + e));
                                    let bottom = egui::Rect::from_min_max(egui::pos2(card_rect.min.x, card_rect.max.y - e), card_rect.max);
                                    let left = egui::Rect::from_min_max(card_rect.min, egui::pos2(card_rect.min.x + e, card_rect.max.y));
                                    let right = egui::Rect::from_min_max(egui::pos2(card_rect.max.x - e, card_rect.min.y), card_rect.max);
                                    let dresp = ui
                                        .interact(top, id.with("t"), egui::Sense::drag())
                                        .union(ui.interact(bottom, id.with("b"), egui::Sense::drag()))
                                        .union(ui.interact(left, id.with("l"), egui::Sense::drag()))
                                        .union(ui.interact(right, id.with("r"), egui::Sense::drag()));
                                    if dresp.hovered() || self.stats_dragging_block == Some(block) {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
                                    }
                                    if dresp.drag_started() {
                                        self.stats_dragging_block = Some(block);
                                    }
                                    // Copy buttons drawn last so they sit on top of
                                    // the edge-drag strips at the corner. Painted
                                    // via interact (not ui.put) so they don't
                                    // allocate layout space and shift later cards.
                                    Self::render_card_copy_buttons(ui, block, card_rect, Some(stats), &char.pc_stats_raw);
                                    block_rects.push((block, card_rect));
                                }
                            }).response;
                            col_rects.push(col_resp.rect);
                        }

                        // Resolve an in-progress drag: highlight the target
                        // column and, on release, move the dragged block into
                        // that column at the drop slot (by pointer.y).
                        if let Some(drag) = self.stats_dragging_block {
                            let pointer = ui.input(|i| i.pointer.interact_pos());
                            let released = ui.input(|i| i.pointer.any_released());
                            // Target column = the one whose x-range contains the
                            // pointer, else the nearest column by center-x.
                            let target_col = pointer.and_then(|p| {
                                col_rects
                                    .iter()
                                    .position(|r| p.x >= r.left() && p.x <= r.right())
                                    .or_else(|| {
                                        col_rects
                                            .iter()
                                            .enumerate()
                                            .min_by(|(_, a), (_, b)| {
                                                (p.x - a.center().x)
                                                    .abs()
                                                    .total_cmp(&(p.x - b.center().x).abs())
                                            })
                                            .map(|(i, _)| i)
                                    })
                            });
                            if let (Some(tc), Some(p)) = (target_col, pointer) {
                                if let Some(r) = col_rects.get(tc) {
                                    ui.painter().rect_stroke(
                                        *r,
                                        8.0,
                                        egui::Stroke::new(2.0_f32, Color32::from_rgb(255, 200, 50)),
                                        egui::StrokeKind::Inside,
                                    );
                                }
                                if released {
                                    // Insert before the first card in the target
                                    // column whose center is below the pointer;
                                    // otherwise append to the column's end.
                                    let before = block_rects
                                        .iter()
                                        .filter(|(b, _)| {
                                            *b != drag && self.stats_block_column[*b] == tc
                                        })
                                        .find(|(_, r)| p.y < r.center().y)
                                        .map(|(b, _)| *b);
                                    self.move_block_to_column(drag, tc, before);
                                }
                            }
                            if released {
                                self.stats_dragging_block = None;
                            } else {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
                                ui.ctx().request_repaint();
                            }
                        }
                    } else {
                        // No decoded stats: still show the derived Attributes,
                        // which depend only on gear/enchants/pet.
                        ui.add_space(col_spacing);
                        ui.vertical(|ui| {
                            ui.set_min_width(col_width);
                            ui.set_max_width(col_width);
                            derived_stats::render_readout(ui, sprite_renderer, &derived, &mut self.theoretical_target_def, &mut self.target_def_text, &char.maxed_flags(), &ctx, true, false, show_breakdown, Some(&identity));
                            ui.add_space(12.0);
                            if char.pc_stats_raw.is_empty() {
                                ui.label(
                                    RichText::new("No stats data available.\n\nPlay this character or refresh from API to load stats.")
                                        .color(Color32::GRAY),
                                );
                            } else {
                                ui.label(
                                    RichText::new("Failed to decode stats data.")
                                        .color(Color32::from_rgb(255, 150, 150)),
                                );
                            }
                        });
                    }
                });
            });

        // Persist measured widths for next frame's per-column width unifying.
        // Repaint once when they shift materially so columns resize promptly.
        if stats.is_some() {
            let w_changed = self
                .stats_block_widths
                .iter()
                .zip(new_widths.iter())
                .any(|(a, b)| (a - b).abs() > 0.5);
            self.stats_block_widths = new_widths;
            if w_changed {
                ui.ctx().request_repaint();
            }
        }

        // Close on Escape
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.viewing_stats_for = None;
        }

        // Persist the stats-page layout if the user changed it this frame.
        let layout_after = (
            self.stats_block_order,
            self.stats_block_column,
            self.category_visible,
            self.dungeon_sort,
            self.stats_show_breakdown,
        );
        if layout_after != layout_before {
            self.pending_edits
                .push(AppAction::SaveCharactersSettings(self.to_settings()));
        }
    }

    /// Overlay the icon-only copy buttons at the top-right corner of a stats
    /// card, painted via [`egui::Ui::interact`] (not `ui.put`) so they never
    /// allocate layout space and shift the cards below. Statistics / Biome Kills
    /// / Dungeons get a "Copy" button (⧉, "two pages" glyph); Dungeons also gets
    /// a "Copy Debug" button (🔧 wrench) that dumps the raw PCStats alignment.
    /// Nothing is drawn for the non-exportable blocks or while collapsed.
    fn render_card_copy_buttons(
        ui: &mut egui::Ui,
        block: usize,
        card_rect: egui::Rect,
        stats: Option<&CharacterStats>,
        pc_stats_raw: &str,
    ) {
        let (key, category) = match block {
            STATS_BLOCK_STATISTICS => ("statistics", "Statistics"),
            STATS_BLOCK_BIOMES => ("biome_kills", "Biome Kills"),
            STATS_BLOCK_DUNGEONS_0 => ("dungeons", "Dungeons"),
            _ => return,
        };
        if derived_stats::category_collapsed(ui.ctx(), key) {
            return;
        }

        let bs = egui::vec2(24.0, 20.0);
        let pad = 6.0;
        let top = card_rect.top() + pad;
        let mut right = card_rect.right() - pad;

        // Dungeons: debug dump sits rightmost, the copy button to its left.
        if block == STATS_BLOCK_DUNGEONS_0 {
            let debug_rect = egui::Rect::from_min_size(egui::pos2(right - bs.x, top), bs);
            if Self::corner_icon_button(ui, ("card_debug", block), debug_rect, "🔧", "Copy Debug")
            {
                ui.ctx().copy_text(debug_order_alignment(pc_stats_raw));
            }
            right -= bs.x + 4.0;
        }

        let copy_rect = egui::Rect::from_min_size(egui::pos2(right - bs.x, top), bs);
        if Self::corner_icon_button(
            ui,
            ("card_copy", block),
            copy_rect,
            "⧉",
            &format!("Copy {}", category),
        ) {
            let text = match block {
                STATS_BLOCK_STATISTICS => Self::format_stats_only_for_excel(stats),
                STATS_BLOCK_BIOMES => Self::format_biomes_for_excel(stats),
                STATS_BLOCK_DUNGEONS_0 => Self::format_dungeon_values_for_excel(stats),
                _ => String::new(),
            };
            ui.ctx().copy_text(text);
        }
    }

    /// Paint a small icon button at an absolute `rect` using `interact` +
    /// painter, so it overlays a card without participating in layout. Returns
    /// whether it was clicked.
    fn corner_icon_button(
        ui: &mut egui::Ui,
        id_src: (&'static str, usize),
        rect: egui::Rect,
        glyph: &str,
        tooltip: &str,
    ) -> bool {
        let resp = ui.interact(rect, ui.make_persistent_id(id_src), egui::Sense::click());
        let visuals = ui.style().interact(&resp);
        ui.painter().rect(
            rect,
            4.0,
            visuals.bg_fill,
            visuals.bg_stroke,
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            glyph,
            egui::FontId::proportional(14.0),
            visuals.text_color(),
        );
        resp.on_hover_text(tooltip).clicked()
    }

    /// Render the Statistics column.
    fn render_stats_column(
        &self,
        ui: &mut egui::Ui,
        stats: &CharacterStats,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        if !derived_stats::collapsible_header(ui, "statistics", "STATISTICS", 16.0) {
            return;
        }
        ui.add_space(4.0);

        egui::Grid::new("stats_grid")
            .num_columns(4)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                Self::stat_row_with_icon(ui, sprite_renderer, "Shots Fired", stats.shots_fired);
                Self::stat_row_with_icon(ui, sprite_renderer, "Hits", stats.hits);
                Self::stat_row_with_icon(ui, sprite_renderer, "Ability Uses", stats.ability_used);
                Self::stat_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Tiles Discovered",
                    stats.tiles_discovered,
                );
                Self::stat_row_with_icon(ui, sprite_renderer, "Teleports", stats.teleports);
                Self::stat_row_with_icon(ui, sprite_renderer, "Potions Drunk", stats.potions_drunk);
                Self::stat_row_with_icon(ui, sprite_renderer, "Kills", stats.kills);
                Self::stat_row_with_icon(ui, sprite_renderer, "Assists", stats.assists);
                Self::stat_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Party Level-ups",
                    stats.party_level_ups,
                );
                Self::stat_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Lesser Gods Kills",
                    stats.lesser_gods_kills,
                );
                Self::stat_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Encounter Kills",
                    stats.encounter_kills,
                );
                Self::stat_row_with_icon(ui, sprite_renderer, "Hero Kills", stats.hero_kills);
                Self::stat_row_with_icon(ui, sprite_renderer, "Critter Kills", stats.critter_kills);
                Self::stat_row_with_icon(ui, sprite_renderer, "Beast Kills", stats.beast_kills);
                Self::stat_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Humanoid Kills",
                    stats.humanoid_kills,
                );
                Self::stat_row_with_icon(ui, sprite_renderer, "Undead Kills", stats.undead_kills);
                Self::stat_row_with_icon(ui, sprite_renderer, "Nature Kills", stats.nature_kills);
                Self::stat_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Construct Kills",
                    stats.construct_kills,
                );
                Self::stat_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Grotesque Kills",
                    stats.grotesque_kills,
                );
                Self::stat_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Structure Kills",
                    stats.structure_kills,
                );
                Self::stat_row_with_icon(ui, sprite_renderer, "God Kills", stats.god_kills);
                Self::stat_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Assists Against Gods",
                    stats.assists_against_gods,
                );
                Self::stat_row_with_icon(ui, sprite_renderer, "Cube Kills", stats.cube_kills);
                Self::stat_row_with_icon(ui, sprite_renderer, "Oryx Kills", stats.oryx_kills);
                Self::stat_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Quests Completed",
                    stats.quests_completed,
                );
                Self::stat_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Minutes Active",
                    stats.minutes_active,
                );
                Self::stat_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Dungeon Types Completed",
                    stats.dungeon_types_completed,
                );
                Self::stat_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Stat Potions Consumed",
                    stats.stat_potions_consumed,
                );
            });
    }

    /// Render the Biomes column.
    fn render_biomes_column(
        &self,
        ui: &mut egui::Ui,
        stats: &CharacterStats,
        sprite_renderer: &mut SpriteRenderer,
        _exaltation: Option<&ClassExaltation>,
    ) {
        if !derived_stats::collapsible_header(ui, "biome_kills", "BIOME ENEMY KILLS", 16.0) {
            return;
        }
        ui.add_space(4.0);

        egui::Grid::new("biomes_grid")
            .num_columns(4)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                Self::biome_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Ruins",
                    stats.biomes.ruins_enemy_kills,
                );
                Self::biome_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Beach",
                    stats.biomes.beach_enemy_kills,
                );
                Self::biome_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Undead Forest",
                    stats.biomes.undead_forest_enemy_kills,
                );
                Self::biome_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Forest",
                    stats.biomes.forest_enemy_kills,
                );
                Self::biome_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Plains",
                    stats.biomes.plains_enemy_kills,
                );
                Self::biome_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Wither",
                    stats.biomes.wither_enemy_kills,
                );
                Self::biome_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Dark Forest",
                    stats.biomes.dark_forest_enemy_kills,
                );
                Self::biome_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Desert",
                    stats.biomes.desert_enemy_kills,
                );
                Self::biome_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Coral Reefs",
                    stats.biomes.coral_reefs_enemy_kills,
                );
                Self::biome_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Sprite Forest",
                    stats.biomes.sprite_forest_enemy_kills,
                );
                Self::biome_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Haunted Hallows",
                    stats.biomes.haunted_hallows_enemy_kills,
                );
                Self::biome_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Shipwreck Cove",
                    stats.biomes.shipwreck_cove_enemy_kills,
                );
                Self::biome_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Dead Church",
                    stats.biomes.dead_church_enemy_kills,
                );
                Self::biome_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Risen Hells",
                    stats.biomes.risen_hells_enemy_kills,
                );
                Self::biome_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Abandoned City",
                    stats.biomes.abandoned_city_enemy_kills,
                );
                Self::biome_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Deep Sea Abyss",
                    stats.biomes.deep_sea_abyss_enemy_kills,
                );
                Self::biome_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Carboniferous",
                    stats.biomes.carboniferous_enemy_kills,
                );
                Self::biome_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Floral Escape",
                    stats.biomes.floral_escape_enemy_kills,
                );
                Self::biome_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Sanguine Forest",
                    stats.biomes.sanguine_forest_enemy_kills,
                );
                Self::biome_row_with_icon(
                    ui,
                    sprite_renderer,
                    "Runic Tundra",
                    stats.biomes.runic_tundra_kills,
                );
            });
    }

    /// Whether a stats block has anything to render for this character. Only
    /// MISC STATS can be empty (a character with no lifetime tallies); all other
    /// blocks always have content when stats are loaded.
    fn stats_block_has_content(block: usize, char: &CachedCharacter) -> bool {
        match block {
            STATS_BLOCK_MISC => {
                char.lone_fighter > 0
                    || char.last_hero_standing > 0
                    || char.most_damage_taken > 0
                    || char.close_calls > 0
            }
            _ => true,
        }
    }

    /// Render the MISC STATS section: lifetime "Lone fighter",
    /// "Last hero standing", and "Close calls" tallies, each with a small dyed
    /// character sprite icon. Shown only when at least one of them is positive.
    fn render_misc_stats(
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        char: &CachedCharacter,
    ) {
        if char.lone_fighter <= 0
            && char.last_hero_standing <= 0
            && char.most_damage_taken <= 0
            && char.close_calls <= 0
        {
            return;
        }
        if !derived_stats::collapsible_header(ui, "misc_stats", "MISC STATS", 16.0) {
            return;
        }
        ui.add_space(4.0);

        // Match the BIOME ENEMY KILLS layout: sprite in the icon column, the
        // label under the biome names, and the count under the kill numbers.
        // Widen the label cell to the longest biome name so the value column
        // lines up with the biome grid above.
        let label_width = ui.fonts_mut(|f| {
            BIOME_DISPLAY_NAMES
                .iter()
                .map(|name| {
                    f.layout_no_wrap(
                        (*name).to_string(),
                        egui::FontId::proportional(14.0),
                        Color32::WHITE,
                    )
                    .rect
                    .width()
                })
                .fold(0.0_f32, f32::max)
        });

        let skin_id = if char.skin > 0 {
            char.skin
        } else {
            char.class_id as i32
        };
        let (tex1, tex2) = (char.tex1, char.tex2);

        // One row per stat: (glow colour or red close-call overlay, label, value,
        // tooltip). `None` glow means the red close-call overlay is used instead.
        enum Icon {
            Glow(Color32),
            CloseCall,
        }
        let rows: Vec<(Icon, &str, i64, &str)> = {
            let mut v = Vec::new();
            if char.lone_fighter > 0 {
                v.push((
                    Icon::Glow(crate::rendering::sprite_renderer::LONE_FIGHTER_GLOW),
                    "Exalt bosses soloed",
                    char.lone_fighter,
                    "Times your character faced and finished a challenging boss solo.",
                ));
            }
            if char.last_hero_standing > 0 {
                v.push((
                    Icon::Glow(crate::rendering::sprite_renderer::LAST_HERO_GLOW),
                    "Exalt bosses finished solo",
                    char.last_hero_standing,
                    "Times your character was the last survivor in the group to finish a challenging boss.",
                ));
            }
            if char.most_damage_taken > 0 {
                v.push((
                    Icon::Glow(crate::rendering::sprite_renderer::MOST_DAMAGE_TAKEN_GLOW),
                    "Most damage taken",
                    char.most_damage_taken,
                    "Times your character took the most damage of the group during a challenging boss.",
                ));
            }
            if char.close_calls > 0 {
                v.push((
                    Icon::CloseCall,
                    "Close calls",
                    char.close_calls,
                    "Times your character dropped below 20% HP (deaths included). \
                     Tracked live by RealmHound; not backfilled from before this feature.",
                ));
            }
            v
        };

        egui::Grid::new("misc_stats_grid")
            .num_columns(3)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                for (icon, label, value, tooltip) in rows {
                    let (rect, icon_resp) =
                        ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                    match icon {
                        Icon::Glow(color) => {
                            sprite_renderer.draw_dyed_outlined_character_sprite_glow(
                                ui, skin_id, rect, 6, tex1, tex2, color, 5, None,
                            );
                        }
                        Icon::CloseCall => {
                            sprite_renderer.draw_dyed_outlined_character_sprite_colored_outline(
                                ui,
                                skin_id,
                                rect,
                                6,
                                tex1,
                                tex2,
                                Some(Color32::from_rgba_unmultiplied(255, 0, 0, 128)),
                                Color32::from_rgb(255, 0, 0),
                            );
                        }
                    }

                    let lbl_resp = ui
                        .scope(|ui| {
                            ui.set_min_width(label_width);
                            ui.label(RichText::new(label).size(14.0).color(Color32::WHITE));
                        })
                        .response;

                    let val_resp = ui.label(
                        RichText::new(Self::format_number(value as i32))
                            .size(14.0)
                            .color(Color32::WHITE),
                    );
                    ui.end_row();

                    (icon_resp | lbl_resp | val_resp).hover_tip(tooltip);
                }
            });
    }

    /// Render a dungeons column (split by index).
    /// column_index: 0 = first half, 1 = second half
    fn render_dungeons_column(
        &mut self,
        ui: &mut egui::Ui,
        stats: &CharacterStats,
        sprite_renderer: &mut SpriteRenderer,
        column_index: usize,
        sort: DungeonSort,
    ) {
        // Both dungeon halves share one collapsible "DUNGEONS" category. When
        // collapsed, draw just the clickable header (which re-expands on click).
        if derived_stats::category_collapsed(ui.ctx(), "dungeons") {
            derived_stats::collapsible_header(ui, "dungeons", "DUNGEONS", 16.0);
            return;
        }

        derived_stats::collapsible_header(ui, "dungeons", "DUNGEONS", 16.0);
        ui.add_space(4.0);

        let portal_map = get_dungeon_portal_map();
        let dungeons = Self::sorted_dungeon_list(&stats.dungeons, sort);

        // Split list in half
        let mid = (dungeons.len() + 1) / 2; // Round up so first column gets extra if odd
        let (start, end) = if column_index == 0 {
            (0, mid)
        } else {
            (mid, dungeons.len())
        };
        let rows = &dungeons[start..end];

        egui::Grid::new(format!("dungeons_grid_{}", column_index))
            .num_columns(4)
            .spacing([8.0, 2.0])
            .show(ui, |ui| {
                for (name, count) in rows.iter() {
                    let tracked = dungeon_tracked(name);
                    let color = if !tracked {
                        // Faded dull red for dungeons we cannot read completions for.
                        Color32::from_rgb(150, 90, 90)
                    } else if *count > 0 {
                        Color32::WHITE
                    } else {
                        Color32::from_rgb(80, 80, 80)
                    };

                    // Icon column
                    let icon_size = 20.0;
                    let (rect, r_icon) = ui.allocate_exact_size(
                        egui::vec2(icon_size, icon_size),
                        egui::Sense::hover(),
                    );
                    if let Some(portal_id) = portal_map.get_portal_id(name) {
                        sprite_renderer.draw_sprite_in_rect(ui, portal_id, rect);
                    }

                    let r_label = ui.label(RichText::new(*name).size(14.0).color(color));
                    let r_value = ui.label(
                        RichText::new(Self::format_number(*count))
                            .size(14.0)
                            .color(color),
                    );
                    let r_fame =
                        Self::dungeon_fame_cell(ui, sprite_renderer, name, *count as i64, color);
                    ui.end_row();

                    if !tracked {
                        r_icon
                            .union(r_label)
                            .union(r_value)
                            .union(r_fame)
                            .hover_tip(UNTRACKED_DUNGEON_TOOLTIP);
                    } else if let Some(bd) = dungeon_tier_breakdown(name, *count as i64) {
                        Self::fame_row_response(ui, [&r_icon, &r_label, &r_value, &r_fame], name)
                            .hover_tip_ui(|ui| Self::fame_tier_tooltip(ui, sprite_renderer, &bd));
                    }
                }
            });
    }

    /// Display name of the active sub-tab, used by the stats-page "Back" label.
    fn tab_display_name(&self) -> &'static str {
        match self.active_tab {
            CharacterTab::Seasonal => "Seasonal",
            CharacterTab::Regular => "Regular",
            CharacterTab::Graveyard => "Graveyard",
        }
    }

    /// Ordered character ids for the active sub-tab, matching the grid's sort so
    /// the stats-page Previous/Next steps through the same list (never crossing
    /// into another sub-tab). The search filter is intentionally ignored so the
    /// whole sub-tab is navigable.
    fn tab_character_ids(&self) -> Vec<i32> {
        let mut chars: Vec<CachedCharacter> = if self.active_tab == CharacterTab::Graveyard {
            self.cache.get_dead_characters().iter().cloned().collect()
        } else {
            let show_seasonal = matches!(self.active_tab, CharacterTab::Seasonal);
            self.cache
                .get_sorted_characters(show_seasonal)
                .iter()
                .map(|c| (*c).clone())
                .collect()
        };
        Self::sort_characters(&mut chars, self.sort_mode);
        chars.into_iter().map(|c| c.char_id).collect()
    }

    /// Reassign `block` to `target_col` and move it within `stats_block_order`
    /// so that, among the blocks in the target column, it lands just before
    /// `before` (or at the end of that column when `before` is `None`). Other
    /// columns keep their relative order.
    fn move_block_to_column(&mut self, block: usize, target_col: usize, before: Option<usize>) {
        self.stats_block_column[block] = target_col;
        let mut order: Vec<usize> = self.stats_block_order.to_vec();
        let Some(from) = order.iter().position(|&x| x == block) else {
            return;
        };
        order.remove(from);
        let insert_at = if let Some(b) = before {
            order.iter().position(|&x| x == b).unwrap_or(order.len())
        } else {
            // Append after the last block already in the target column.
            order
                .iter()
                .rposition(|&x| self.stats_block_column[x] == target_col)
                .map(|p| p + 1)
                .unwrap_or(order.len())
        };
        order.insert(insert_at, block);
        self.stats_block_order.copy_from_slice(&order);
    }

    /// Build the dungeon list ordered by the given sort. Difficulty sorts place
    /// unknown-difficulty dungeons last with an alphabetical tie-break, matching
    /// the Trophy Hall convention.
    fn sorted_dungeon_list(
        dungeons: &realmhound_core::api::DungeonStats,
        sort: DungeonSort,
    ) -> Vec<(&'static str, i32)> {
        use realmhound_core::assets::dungeon_difficulty;
        let mut list = get_dungeon_list(dungeons);
        match sort {
            DungeonSort::Default => {}
            DungeonSort::DifficultyAsc => {
                list.sort_by(|a, b| {
                    let da = dungeon_difficulty(a.0).unwrap_or(f32::MAX);
                    let db = dungeon_difficulty(b.0).unwrap_or(f32::MAX);
                    da.total_cmp(&db).then_with(|| a.0.cmp(b.0))
                });
            }
            DungeonSort::DifficultyDesc => {
                list.sort_by(|a, b| {
                    let da = dungeon_difficulty(a.0).unwrap_or(-1.0);
                    let db = dungeon_difficulty(b.0).unwrap_or(-1.0);
                    db.total_cmp(&da).then_with(|| a.0.cmp(b.0))
                });
            }
            DungeonSort::MostCompleted => {
                list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
            }
        }
        list
    }

    /// Render the Dungeon Collections column (with exaltation progress below if available).
    /// Shows all collections with progress and missing dungeon portals.
    fn render_collections_column(
        &self,
        ui: &mut egui::Ui,
        stats: &CharacterStats,
        sprite_renderer: &mut SpriteRenderer,
        base_fame: i64,
    ) {
        if !derived_stats::collapsible_header(
            ui,
            "dungeon_collections",
            "DUNGEON COLLECTIONS",
            16.0,
        ) {
            return;
        }
        ui.add_space(4.0);

        let portal_map = get_dungeon_portal_map();
        let portal_size = 20.0;

        // Pre-measure the widest "name + progress" so every collection's fame
        // icon can sit at the same x (aligned like the DUNGEONS column). The "+"
        // sits in a fixed slot to the left of the icon, so icons stay aligned
        // whether or not a row shows a predicted "+".
        let spacing = ui.spacing().item_spacing.x;
        let text_w = |ui: &mut egui::Ui, s: &str, size: f32| {
            ui.fonts_mut(|f| {
                f.layout_no_wrap(
                    s.to_string(),
                    egui::FontId::proportional(size),
                    Color32::WHITE,
                )
                .size()
                .x
            })
        };
        let check_w = text_w(ui, "✅", 14.0);
        let mut fame_col_x = 0.0f32;
        for collection in DUNGEON_COLLECTIONS.iter() {
            let (completed, total, missing) = collection.calculate_completion(&stats.dungeons);
            let mut w = text_w(ui, collection.name, 14.0)
                + spacing
                + text_w(ui, &format!("{}/{}", completed, total), 14.0);
            if missing.is_empty() {
                w += check_w + spacing;
            }
            fame_col_x = fame_col_x.max(w);
        }
        fame_col_x += 12.0;

        for collection in DUNGEON_COLLECTIONS.iter() {
            let (completed, total, mut missing) = collection.calculate_completion(&stats.dungeons);
            let is_complete = missing.is_empty();

            // Order remaining portals easiest-first (unknown difficulty last).
            missing.sort_by(|a, b| {
                let da =
                    realmhound_core::assets::dungeon_difficulty(a.name).unwrap_or(f32::INFINITY);
                let db =
                    realmhound_core::assets::dungeon_difficulty(b.name).unwrap_or(f32::INFINITY);
                da.total_cmp(&db)
            });

            // Row 1: Checkmark (if complete), Name and progress
            ui.horizontal(|ui| {
                let row_left = ui.cursor().min.x;
                // Checkmark for completed
                if is_complete {
                    ui.label(RichText::new("✅").size(14.0));
                }

                // Collection name
                let name_color = if is_complete {
                    exalt_colors::GREEN // #7ac142
                } else {
                    Color32::WHITE
                };
                ui.label(RichText::new(collection.name).size(14.0).color(name_color));

                // Progress
                let progress_color = if is_complete {
                    exalt_colors::GREEN // #7ac142
                } else if completed > 0 {
                    Color32::from_rgb(200, 200, 100) // Yellow-ish for partial
                } else {
                    Color32::from_rgb(150, 150, 150) // Gray for none
                };
                ui.label(RichText::new(format!("{}/{}", completed, total)).size(14.0).color(progress_color));

                // Fame reward column, aligned by the fame icon like the DUNGEONS
                // column. Completed collections show the earned reward in bright
                // orange with an opaque fame icon; incomplete ones show the
                // predicted reward in grey with a semi-transparent icon, a "+"
                // in a fixed slot to the left of the icon, and a hover tooltip.
                let bonus = collection_bonus(base_fame, collection);
                if bonus > 0 {
                    let pad = fame_col_x - (ui.cursor().min.x - row_left);
                    if pad > 0.0 {
                        ui.add_space(pad);
                    }
                    ui.spacing_mut().item_spacing.x = 2.0;

                    // Fixed "+" slot so icons align across predicted/completed rows.
                    let plus_w = 10.0;
                    let (prect, presp) =
                        ui.allocate_exact_size(egui::vec2(plus_w, 13.0), egui::Sense::hover());
                    if !is_complete {
                        ui.painter().text(
                            prect.right_center(),
                            egui::Align2::RIGHT_CENTER,
                            "+",
                            egui::FontId::proportional(13.0),
                            Color32::from_gray(140),
                        );
                    }

                    let (rect, icon) =
                        ui.allocate_exact_size(egui::Vec2::splat(13.0), egui::Sense::hover());
                    if ui.is_rect_visible(rect) {
                        if is_complete {
                            sprite_renderer.draw_sprite_in_rect(ui, FAME_SPRITE_ID, rect);
                        } else {
                            sprite_renderer.draw_sprite_in_rect_tinted(
                                ui,
                                FAME_SPRITE_ID,
                                rect,
                                Color32::from_white_alpha(110),
                            );
                        }
                    }

                    let value_color = if is_complete {
                        Color32::from_rgb(255, 200, 100)
                    } else {
                        Color32::from_gray(140)
                    };
                    let value = ui.label(RichText::new(format_fame(bonus)).size(13.0).color(value_color));

                    if !is_complete {
                        value.union(icon).union(presp).hover_tip(
                            "Predicted fame reward for completing this dungeon collection. See below for the dungeons left to complete",
                        );
                    }
                }
            });

            // Row 2: Missing dungeon portals (wrapping) - only if incomplete
            if !is_complete {
                // Fixed 10-portal-per-row grid so long collections wrap instead
                // of stretching the column.
                egui::Grid::new(("missing_portals", collection.name))
                    .num_columns(12)
                    .min_col_width(portal_size)
                    .max_col_width(portal_size)
                    .spacing([3.0, 3.0])
                    .show(ui, |ui| {
                        for (i, dungeon) in missing.iter().enumerate() {
                            let (rect, response) = ui.allocate_exact_size(
                                egui::vec2(portal_size, portal_size),
                                egui::Sense::hover(),
                            );

                            // Draw portal sprite
                            if let Some(portal_id) = portal_map.get_portal_id(dungeon.name) {
                                sprite_renderer.draw_sprite_in_rect(ui, portal_id, rect);
                            } else {
                                // Fallback: draw a placeholder
                                ui.painter().rect_filled(
                                    rect,
                                    2.0,
                                    crate::ui_colors::slot_stroke(ui.visuals()),
                                );
                                ui.painter().text(
                                    rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "?",
                                    egui::FontId::proportional(10.0),
                                    Color32::GRAY,
                                );
                            }

                            attach_dungeon_tooltip(
                                response,
                                sprite_renderer,
                                dungeon.name,
                                dungeon.name,
                            );

                            if (i + 1) % 12 == 0 {
                                ui.end_row();
                            }
                        }
                    });
            }

            ui.add_space(4.0);
        }
    }

    /// Render the label edit modal if editing_label is set.
    fn render_label_edit_modal(&mut self, ui: &mut egui::Ui, shadcn: &Shadcn) {
        let Some(char_id) = self.editing_label else {
            return;
        };

        // Find the character (living or dead, so graveyard chars resolve too)
        let char_data = self.cache.find_any_character(char_id).cloned();
        let Some(char) = char_data else {
            self.editing_label = None;
            self.label_dialog_open = false;
            return;
        };

        // Drive the dialog from persistent state and let it own its close so the
        // dialog's internal open tracking stays in sync -- otherwise a stale
        // "just opened" state makes the opening click read as an outside click,
        // closing it instantly and requiring a second click on the next open.
        let mut open = self.label_dialog_open;
        let mut close_requested = false;
        let focus_now = self.label_dialog_focus;
        let result = shadcn.dialog(
            ui,
            "label_edit_dialog",
            &mut open,
            "Edit Label",
            300.0,
            200.0,
            |ui| {
                ui.label(
                    RichText::new(format!("{} (ID: #{})", char.class_name(), char.char_id))
                        .color(Color32::GRAY)
                        .small(),
                );

                ui.add_space(12.0);

                // Label input
                ui.label(format!("Label ({} symbols):", MAX_LABEL_LEN));
                let response = shadcn.text_edit_counted(
                    ui,
                    &mut self.label_edit_buffer,
                    MAX_LABEL_LEN,
                    268.0,
                    None,
                );

                // Auto-focus the text field only on the first frame.
                if focus_now {
                    response.request_focus();
                }

                // Enter key saves
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.cache
                        .set_label(char_id, self.label_edit_buffer.clone());
                    self.mark_needs_save();
                    self.pending_edits.push(AppAction::SetCharacterLabel {
                        char_id,
                        label: self.label_edit_buffer.clone(),
                    });
                    close_requested = true;
                }

                ui.add_space(16.0);

                // Buttons
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if shadcn.button(ui, "Cancel").clicked() {
                            close_requested = true;
                        }

                        if shadcn.button(ui, "OK").clicked() {
                            self.cache
                                .set_label(char_id, self.label_edit_buffer.clone());
                            self.mark_needs_save();
                            self.pending_edits.push(AppAction::SetCharacterLabel {
                                char_id,
                                label: self.label_edit_buffer.clone(),
                            });
                            close_requested = true;
                        }
                    });
                });
            },
        );
        self.label_dialog_focus = false;
        if close_requested {
            open = false;
        }
        self.label_dialog_open = open;
        // Keep the modal mounted until the dialog has fully closed (including its
        // fade-out) so it can persist its closed state; only then drop the edit.
        if !self.label_dialog_open && result.is_none() {
            self.editing_label = None;
        }
    }

    /// Helper to render a stat row with an icon in the grid.
    fn stat_row_with_icon(
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        label: &str,
        value: i32,
    ) {
        let icon_size = 20.0;

        // Column 1: Icon
        let (rect, r_icon) =
            ui.allocate_exact_size(egui::vec2(icon_size, icon_size), egui::Sense::hover());

        // Special case: "Dungeon Types Completed" uses embedded icon
        if label == "Dungeon Types Completed" {
            sprite_renderer.draw_embedded_icon(ui, EmbeddedIcon::DungeonTypesCompleted, rect);
        } else if let Some(icon_id) = get_stat_icon(label) {
            sprite_renderer.draw_sprite_in_rect(ui, icon_id, rect);
        }
        // No icon case: rect already allocated for alignment

        // Column 2: Label
        let r_label = ui.label(RichText::new(label).size(14.0));

        // Column 3: Value
        let r_value = ui.label(
            RichText::new(Self::format_number(value))
                .size(14.0)
                .color(Color32::WHITE),
        );

        // Column 4: Earned fame bonus (orange), for verifying against the game.
        // Statistics that can never award fame show a dash instead of a blank;
        // fame-granting rows with nothing earned yet preview their first tier.
        let breakdown = stat_tier_breakdown(label, value as i64);
        let r_fame = if breakdown.is_none() {
            Self::fame_dash_cell(ui, Color32::from_gray(140))
        } else {
            let earned = stat_line_bonus(label, value as i64);
            if earned > 0 {
                Self::fame_cell(ui, sprite_renderer, earned)
            } else {
                Self::predicted_fame_cell(ui, sprite_renderer, stat_first_tier_bonus(label))
            }
        };
        ui.end_row();

        // Hovering any cell in a fame-granting row shows the tier breakdown.
        if let Some(bd) = breakdown {
            Self::fame_row_response(ui, [&r_icon, &r_label, &r_value, &r_fame], label)
                .hover_tip_ui(|ui| Self::fame_tier_tooltip(ui, sprite_renderer, &bd));
        }
    }

    /// Render the "no fame possible" dash for a fame column, indented by the
    /// same gutter + icon width as the earned/predicted cells so the dash lines
    /// up under the fame icon column instead of the row's value.
    fn fame_dash_cell(ui: &mut egui::Ui, color: Color32) -> egui::Response {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.allocate_exact_size(
                egui::vec2(FAME_PLUS_GUTTER + 2.0 + 14.0, 14.0),
                egui::Sense::hover(),
            );
            ui.label(RichText::new("-").size(14.0).color(color));
        })
        .response
    }

    /// Render the earned-fame cell (orange fame sprite + value) for a bonus row.
    /// Renders an empty cell when the row grants no fame so the grid stays aligned.
    fn fame_cell(
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        fame: i64,
    ) -> egui::Response {
        if fame <= 0 {
            return ui.label("");
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            // Empty leading gutter so the fame icon lines up with predicted rows,
            // which reserve the same space for their "+" prefix.
            ui.allocate_exact_size(egui::vec2(FAME_PLUS_GUTTER, 14.0), egui::Sense::hover());
            let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(14.0), egui::Sense::hover());
            if ui.is_rect_visible(rect) {
                sprite_renderer.draw_sprite_in_rect(ui, FAME_SPRITE_ID, rect);
            }
            ui.label(
                RichText::new(Self::format_number(fame as i32))
                    .size(14.0)
                    .color(Color32::from_rgb(255, 200, 100)),
            );
        })
        .response
    }

    /// Fame cell for a dungeon row. Returns the cell response so the grid stays
    /// aligned and the row hover target can include it.
    ///
    /// - Dungeons that never award fame: a dash in the row's colour.
    /// - Fame dungeons with completions: the earned reward (bright orange).
    /// - Fame dungeons with 0 completions: the greyed, semi-transparent
    ///   predicted first-tier reward with a "+", matching the collections style.
    fn dungeon_fame_cell(
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        name: &str,
        count: i64,
        color: Color32,
    ) -> egui::Response {
        if !dungeon_awards_fame(name) {
            return Self::fame_dash_cell(ui, color);
        }
        let earned = dungeon_completion_bonus(name, count);
        if earned > 0 {
            return Self::fame_cell(ui, sprite_renderer, earned);
        }
        Self::predicted_fame_cell(ui, sprite_renderer, dungeon_first_tier_bonus(name))
    }

    /// Render a greyed, semi-transparent predicted fame cell with a leading "+"
    /// (the first-tier reward not yet earned). Used by dungeon, statistic and
    /// biome rows so every fame column previews its potential first-tier reward
    /// instead of showing a blank cell.
    fn predicted_fame_cell(
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        predicted: i64,
    ) -> egui::Response {
        if predicted <= 0 {
            return ui.label("");
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            // Reserve a leading gutter for the "+" so it sits inside the fame
            // column instead of hanging into the (potentially wide) value column,
            // and so the icon lines up with the earned rows' fame icon.
            let (plus_rect, _) =
                ui.allocate_exact_size(egui::vec2(FAME_PLUS_GUTTER, 14.0), egui::Sense::hover());
            ui.painter().text(
                egui::pos2(plus_rect.right(), plus_rect.center().y),
                egui::Align2::RIGHT_CENTER,
                "+",
                egui::FontId::proportional(14.0),
                Color32::from_gray(140),
            );
            let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(14.0), egui::Sense::hover());
            if ui.is_rect_visible(rect) {
                sprite_renderer.draw_sprite_in_rect_tinted(
                    ui,
                    FAME_SPRITE_ID,
                    rect,
                    Color32::from_white_alpha(110),
                );
            }
            ui.label(
                RichText::new(Self::format_number(predicted as i32))
                    .size(14.0)
                    .color(Color32::from_gray(140)),
            );
        })
        .response
    }
    /// frame, so hovering anywhere on the row (including the gaps between cells)
    /// shows the tier tooltip and the row reads as interactive.
    ///
    /// The frame's right edge is fixed at the fame column's start plus room for a
    /// 5-digit reward, so every row's frame is the same width regardless of how
    /// many digits that row's number has (the grid keeps the column start aligned).
    fn fame_row_response(
        ui: &mut egui::Ui,
        cells: [&egui::Response; 4],
        salt: &str,
    ) -> egui::Response {
        let left = cells[0].rect.left();
        let top = cells
            .iter()
            .map(|c| c.rect.top())
            .fold(f32::INFINITY, f32::min);
        let bottom = cells
            .iter()
            .map(|c| c.rect.bottom())
            .fold(f32::NEG_INFINITY, f32::max);

        // Fame column start (grid-aligned, so identical across rows) + fixed room
        // for the fame icon and a 5-digit number.
        let sample = ui.fonts_mut(|f| {
            f.layout_no_wrap(
                "99,999".to_string(),
                egui::FontId::proportional(14.0),
                Color32::WHITE,
            )
            .size()
            .x
        });
        let fame_width = 14.0 + 2.0 + sample;
        let right = (cells[3].rect.left() + fame_width).max(cells[3].rect.right());

        let row_rect = egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, bottom))
            .expand2(egui::vec2(5.0, 2.0));
        let id = ui.make_persistent_id(("fame_row", salt));
        let resp = ui.interact(row_rect, id, egui::Sense::hover());
        if resp.hovered() {
            ui.painter().rect_stroke(
                row_rect,
                5.0,
                egui::Stroke::new(1.5_f32, Color32::from_rgb(150, 180, 230)),
                egui::StrokeKind::Outside,
            );
        }
        resp
    }

    /// Draw a small filled/outlined diamond node (used by the fame tier tooltip).
    fn draw_diamond(
        painter: &egui::Painter,
        c: egui::Pos2,
        r: f32,
        fill: Color32,
        stroke: egui::Stroke,
    ) {
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(c.x, c.y - r),
                egui::pos2(c.x + r, c.y),
                egui::pos2(c.x, c.y + r),
                egui::pos2(c.x - r, c.y),
            ],
            fill,
            stroke,
        ));
    }

    /// Render the fame-bonus tier tooltip shown when hovering a fame row. Styled
    /// after the Exaltation "Mastery" popup: a header band with the category name
    /// and total earned fame, then a vertical diamond track of tiers. Achieved
    /// tiers are bright orange with an opaque fame icon; the single next tier is
    /// grey with a semi-transparent icon. The connector to the next tier fills
    /// in proportion to progress toward its threshold.
    fn fame_tier_tooltip(
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        bd: &FameTierBreakdown,
    ) {
        const ORANGE: Color32 = Color32::from_rgb(255, 200, 100);
        let grey = Color32::from_gray(140);
        let dim = Color32::from_gray(70);
        let tw = 286.0;
        ui.set_max_width(tw);
        ui.spacing_mut().item_spacing.y = 3.0;

        // Header band (name left, total earned fame right), bleeding into margins.
        let margin = ui.style().spacing.menu_margin;
        let inset = ui.visuals().window_stroke.width;
        let surf = ExaltSurfaces::from_visuals(ui.visuals()).fill;
        let (content, _) = ui.allocate_exact_size(egui::vec2(tw, 26.0), egui::Sense::hover());
        let band = egui::Rect::from_min_max(
            egui::pos2(
                content.left() - margin.left as f32 + inset,
                content.top() - margin.top as f32 + inset,
            ),
            egui::pos2(
                content.right() + margin.right as f32 - inset,
                content.bottom(),
            ),
        );
        let cr = ui.visuals().menu_corner_radius;
        let top_round = egui::CornerRadius {
            nw: cr.nw,
            ne: cr.ne,
            sw: 0,
            se: 0,
        };
        // Total earned fame, drawn as "[icon] number" (icon before the number,
        // matching the spreadsheet columns and the tier rows). The fame-icon
        // column is placed so the widest number in the tooltip - the header
        // total or any tier reward - still fits inside the right boundary. A
        // small total (e.g. 0 completions) must not push a large first-tier
        // reward past the edge.
        let total_str = Self::format_number(bd.total as i32);
        let total_w = ui.fonts_mut(|f| {
            f.layout_no_wrap(total_str.clone(), egui::FontId::proportional(15.0), ORANGE)
                .size()
                .x
        });
        let mut num_w = total_w;
        for tier in &bd.tiers {
            let w = ui.fonts_mut(|f| {
                f.layout_no_wrap(
                    Self::format_number(tier.reward as i32),
                    egui::FontId::proportional(14.0),
                    ORANGE,
                )
                .size()
                .x
            });
            num_w = num_w.max(w);
        }
        let num_left = content.right() - 4.0 - num_w;
        let icon_center_x = num_left - 2.0 - 6.5;
        {
            let painter = ui.painter().with_clip_rect(band);
            painter.rect_filled(band, top_round, surf);
            painter.text(
                egui::pos2(content.left(), content.center().y),
                egui::Align2::LEFT_CENTER,
                &bd.category,
                egui::FontId::proportional(16.0),
                Color32::WHITE,
            );
            painter.text(
                egui::pos2(num_left, content.center().y),
                egui::Align2::LEFT_CENTER,
                total_str,
                egui::FontId::proportional(15.0),
                ORANGE,
            );
        }
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(icon_center_x, content.center().y),
            egui::Vec2::splat(13.0),
        );
        sprite_renderer.draw_sprite_in_rect(ui, FAME_SPRITE_ID, icon_rect);
        ui.add_space(4.0);

        // Tier rows. The fame icon of every tier is painted at a fixed x so it
        // sits directly under the header's fame icon; the reward number follows
        // to its right and the next (grey) tier's "+" is painted just left of it.
        // A left gutter reserves space for the vertical track painted afterwards.
        let gutter = 24.0;
        let gutter_x = content.left() + gutter * 0.5;
        let mut centres: Vec<f32> = Vec::with_capacity(bd.tiers.len());
        for tier in &bd.tiers {
            let text_color = if tier.achieved { ORANGE } else { grey };
            let row_resp = ui
                .horizontal(|ui| {
                    ui.add_space(gutter);
                    ui.label(RichText::new(&tier.name).size(14.0).color(text_color));
                })
                .response;
            let cy = row_resp.rect.center().y;
            centres.push(cy);

            let irect = egui::Rect::from_center_size(
                egui::pos2(icon_center_x, cy),
                egui::Vec2::splat(13.0),
            );
            if tier.achieved {
                sprite_renderer.draw_sprite_in_rect(ui, FAME_SPRITE_ID, irect);
            } else {
                sprite_renderer.draw_sprite_in_rect_tinted(
                    ui,
                    FAME_SPRITE_ID,
                    irect,
                    Color32::from_white_alpha(90),
                );
                ui.painter().text(
                    egui::pos2(irect.left() - 3.0, cy),
                    egui::Align2::RIGHT_CENTER,
                    "+",
                    egui::FontId::proportional(14.0),
                    text_color,
                );
            }
            ui.painter().text(
                egui::pos2(irect.right() + 2.0, cy),
                egui::Align2::LEFT_CENTER,
                Self::format_number(tier.reward as i32),
                egui::FontId::proportional(14.0),
                text_color,
            );
        }

        // Paint the vertical track over the reserved gutter. Draw every connector
        // segment first, then the diamonds on top, so a grey (unfilled) connector
        // can never paint over an achieved tier's diamond.
        let painter = ui.painter();
        for i in 1..centres.len() {
            let c = egui::pos2(gutter_x, centres[i]);
            let a = egui::pos2(gutter_x, centres[i - 1]);
            if bd.tiers[i].achieved {
                painter.line_segment([a, c], egui::Stroke::new(3.0_f32, ORANGE));
            } else {
                let f = bd.next_progress.unwrap_or(0.0);
                let mid = a + (c - a) * f;
                painter.line_segment([a, mid], egui::Stroke::new(3.0_f32, ORANGE));
                painter.line_segment([mid, c], egui::Stroke::new(3.0_f32, dim));
            }
        }
        for i in 0..centres.len() {
            let c = egui::pos2(gutter_x, centres[i]);
            if bd.tiers[i].achieved {
                Self::draw_diamond(painter, c, 6.0, ORANGE, egui::Stroke::NONE);
            } else {
                Self::draw_diamond(painter, c, 6.0, dim, egui::Stroke::new(1.5_f32, grey));
            }
        }

        if let Some(remaining) = bd.next_remaining {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_space(gutter);
                ui.label(
                    RichText::new(format!(
                        "{} until the next tier",
                        Self::format_number(remaining as i32)
                    ))
                    .size(12.0)
                    .color(grey),
                );
            });
        }

        if bd.show_cap_note {
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!(
                    "{} tier fame bonuses cap at 20 repetitions.",
                    bd.top_tier_name
                ))
                .size(11.0)
                .color(grey),
            );
        }
    }

    /// Helper to render a biome kill row with beacon icon (dimmed if zero).
    fn biome_row_with_icon(
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        biome_name: &str,
        value: i32,
    ) {
        let color = if value > 0 {
            Color32::WHITE
        } else {
            Color32::from_rgb(80, 80, 80)
        };

        // Column 1: Icon (sized to roughly match the row's 14pt text so icons
        // don't crowd/overlap each other with only 2px of row spacing).
        let icon_size = 16.0;
        let (rect, r_icon) =
            ui.allocate_exact_size(egui::vec2(icon_size, icon_size), egui::Sense::hover());
        if let Some(icon_id) = get_biome_icon(biome_name) {
            sprite_renderer.draw_sprite_in_rect(ui, icon_id, rect);
        }

        // Column 2: Label
        let r_label = ui.label(RichText::new(biome_name).size(14.0).color(color));

        // Column 3: Value
        let r_value = ui.label(
            RichText::new(Self::format_number(value))
                .size(14.0)
                .color(color),
        );

        // Column 4: Earned fame bonus (orange); biomes with no kills yet preview
        // their first-tier reward instead of showing a blank cell.
        let earned = biome_kill_bonus(value as i64);
        let r_fame = if earned > 0 {
            Self::fame_cell(ui, sprite_renderer, earned)
        } else {
            Self::predicted_fame_cell(ui, sprite_renderer, biome_first_tier_bonus())
        };
        ui.end_row();

        let bd = biome_tier_breakdown(biome_name, value as i64);
        Self::fame_row_response(ui, [&r_icon, &r_label, &r_value, &r_fame], biome_name)
            .hover_tip_ui(|ui| Self::fame_tier_tooltip(ui, sprite_renderer, &bd));
    }

    /// Format a number with thousand separators.
    fn format_number(n: i32) -> String {
        let s = n.to_string();
        let mut result = String::new();
        for (i, c) in s.chars().rev().enumerate() {
            if i > 0 && i % 3 == 0 {
                result.insert(0, ',');
            }
            result.insert(0, c);
        }
        result
    }

    /// Format dungeon values only for Excel (in game order, one value per line).
    fn format_dungeon_values_for_excel(stats: Option<&CharacterStats>) -> String {
        let Some(stats) = stats else {
            return String::new();
        };

        let dungeons = get_dungeon_list(&stats.dungeons);

        dungeons
            .iter()
            .map(|(_, count)| count.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Format only the 28 stats values for Excel (one value per line).
    fn format_stats_only_for_excel(stats: Option<&CharacterStats>) -> String {
        let Some(stats) = stats else {
            return String::new();
        };

        let values = vec![
            stats.shots_fired,
            stats.hits,
            stats.ability_used,
            stats.tiles_discovered,
            stats.teleports,
            stats.potions_drunk,
            stats.kills,
            stats.assists,
            stats.party_level_ups,
            stats.lesser_gods_kills,
            stats.encounter_kills,
            stats.hero_kills,
            stats.critter_kills,
            stats.beast_kills,
            stats.humanoid_kills,
            stats.undead_kills,
            stats.nature_kills,
            stats.construct_kills,
            stats.grotesque_kills,
            stats.structure_kills,
            stats.god_kills,
            stats.assists_against_gods,
            stats.cube_kills,
            stats.oryx_kills,
            stats.quests_completed,
            stats.minutes_active,
            stats.dungeon_types_completed,
            stats.stat_potions_consumed,
        ];

        values
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Format only the 20 biome kills values for Excel (one value per line).
    fn format_biomes_for_excel(stats: Option<&CharacterStats>) -> String {
        let Some(stats) = stats else {
            return String::new();
        };

        let values = vec![
            stats.biomes.ruins_enemy_kills,
            stats.biomes.beach_enemy_kills,
            stats.biomes.undead_forest_enemy_kills,
            stats.biomes.forest_enemy_kills,
            stats.biomes.plains_enemy_kills,
            stats.biomes.wither_enemy_kills,
            stats.biomes.dark_forest_enemy_kills,
            stats.biomes.desert_enemy_kills,
            stats.biomes.coral_reefs_enemy_kills,
            stats.biomes.sprite_forest_enemy_kills,
            stats.biomes.haunted_hallows_enemy_kills,
            stats.biomes.shipwreck_cove_enemy_kills,
            stats.biomes.dead_church_enemy_kills,
            stats.biomes.risen_hells_enemy_kills,
            stats.biomes.abandoned_city_enemy_kills,
            stats.biomes.deep_sea_abyss_enemy_kills,
            stats.biomes.carboniferous_enemy_kills,
            stats.biomes.floral_escape_enemy_kills,
            stats.biomes.sanguine_forest_enemy_kills,
            stats.biomes.runic_tundra_kills,
        ];

        values
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Default for CharactersPanel {
    fn default() -> Self {
        Self::with_cache(CharacterCache::new())
    }
}

impl Panel for CharactersPanel {
    fn show(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext) -> Vec<AppAction> {
        let mut actions = Vec::new();

        if let Some(action) = self.render(ui, ctx.access_token, ctx.sprite_renderer, ctx.shadcn) {
            match action {
                CharacterAction::ViewLoot(char_id, char_name, icon_id) => {
                    actions.push(AppAction::ViewCharacterLoot {
                        char_id,
                        char_name,
                        icon_id,
                    });
                    actions.push(AppAction::SwitchTab(ActiveTab::LootHistory));
                }
                CharacterAction::ViewFights(char_id, char_name) => {
                    // The app switches to the Combat History tab as part of
                    // handling this action.
                    actions.push(AppAction::ViewCharacterFights { char_id, char_name });
                }
            }
        }

        // Signal cache change if modified during this frame
        if self.needs_save {
            actions.push(AppAction::CharacterCacheChanged);
            self.needs_save = false;
        }

        // Route UI-origin edits (label/remove/move) to the processor.
        actions.append(&mut self.pending_edits);

        actions
    }

    fn handle_event(
        &mut self,
        event: &realmhound_core::GameEvent,
        session: &realmhound_core::GameSession,
    ) -> Vec<AppAction> {
        match event {
            realmhound_core::GameEvent::CharacterDied {
                char_id,
                ref killed_by,
                total_fame,
                gravestone_type,
            } => {
                self.mark_dead(*char_id, killed_by, *total_fame, Some(*gravestone_type));
            }
            realmhound_core::GameEvent::CharacterXmlReceived { ref xml } => {
                // Only ingest the character list from a verified main-account
                // connection so mule/alt accounts don't populate the pages.
                if session.connection.is_account_verified() {
                    self.parse_character_xml(xml);
                }
            }
            realmhound_core::GameEvent::SeasonalStatusReceived {
                char_id,
                is_seasonal,
            } => {
                self.cache.update_seasonal_status(*char_id, *is_seasonal);
                self.recompute_tab_counts();
            }
            realmhound_core::GameEvent::ExaltationUpdated {
                class_type,
                ref exaltation,
            } => {
                // Gated on a verified main account so a mule/alt account can't
                // overwrite the main's exaltation progress.
                if session.connection.is_account_verified() {
                    let changed = self
                        .exaltation_stats
                        .get(class_type)
                        .map(|existing| existing != exaltation)
                        .unwrap_or(true);

                    if changed {
                        self.exaltation_stats
                            .insert(*class_type, exaltation.clone());
                        self.cache
                            .exaltation_stats
                            .insert(*class_type, exaltation.clone());
                        self.needs_save = true;
                    }
                }
            }
            _ => {}
        }

        // Signal cache change if modified during event handling
        if self.needs_save {
            self.needs_save = false;
            vec![AppAction::CharacterCacheChanged]
        } else {
            vec![]
        }
    }
}

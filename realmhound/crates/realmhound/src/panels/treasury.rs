//! Treasury panel - consolidated view of all owned items across all storage locations.
//!
//! Reads from `CharacterCache` (characters + pets) and `LiveVaultData` (vault
//! storage) to display every item the player owns, grouped into collapsible
//! storage sections with a sticky header area for future aggregated totals.

use crate::ui_ext::HoverTooltipExt;
use std::collections::{HashMap, HashSet};

use eframe::egui::{self, collapsing_header::CollapsingState, Color32, RichText, ScrollArea};
use realmhound_core::assets::{
    get_asset_manager, AssetManager, ItemCategorizer, ItemCategory, ItemSubCategory, ObjectAsset,
};
use realmhound_core::vault::{
    CachedCharacter, CachedPet, CharacterCache, CharacterItem, LiveVaultData, LiveVaultItem,
    LiveVaultStorage,
};

use crate::panels::character_card::{
    CharacterCardWidget, CARD_CONTENT_WIDTH, CARD_PADDING, CARD_SPACING,
};
use crate::panels::pet_card::PetCardWidget;
use crate::panels::{AppAction, Panel, PanelContext};
use crate::rendering::SpriteRenderer;
use crate::shadcn_ui::Shadcn;
use crate::tab_icons::{get_treasury_section_icon, TAB_ICON_SIZE};
use crate::ui_colors::{REGULAR_COLOR, SEASONAL_COLOR};

// ---------------------------------------------------------------------------
// TreasurySection
// ---------------------------------------------------------------------------

/// Storage sections displayed in the Treasury tab, in their default order.
///
/// Each variant maps to a distinct data source and seasonal flag. The display
/// order is stored as a `Vec<TreasurySection>` on `TreasuryPanel` so it can be
/// reordered via settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TreasurySection {
    SeasonalCharacters,
    SeasonalVault,
    SeasonalMaterials,
    SeasonalPotionRack,
    SeasonalPetInventories,
    SeasonalGiftChests,
    RegularCharacters,
    RegularVault,
    RegularMaterials,
    RegularPotionRack,
    RegularPetInventories,
    RegularGiftChest,
    SeasonalSpoils,
}

impl TreasurySection {
    /// Human-readable display name for the section header.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::SeasonalCharacters => "Seasonal Characters",
            Self::SeasonalVault => "Seasonal Vault",
            Self::SeasonalMaterials => "Seasonal Materials",
            Self::SeasonalPotionRack => "Seasonal Potion Rack",
            Self::SeasonalPetInventories => "Seasonal Pet Inventories",
            Self::SeasonalGiftChests => "Seasonal Gift Chests",
            Self::RegularCharacters => "Regular Characters",
            Self::RegularVault => "Regular Vault",
            Self::RegularMaterials => "Regular Materials",
            Self::RegularPotionRack => "Regular Potion Rack",
            Self::RegularPetInventories => "Regular Pet Inventories",
            Self::RegularGiftChest => "Regular Gift Chest",
            Self::SeasonalSpoils => "Seasonal Spoils",
        }
    }

    /// Default display order matching the design document (Section 4.3).
    pub fn default_order() -> Vec<TreasurySection> {
        vec![
            Self::SeasonalCharacters,
            Self::SeasonalVault,
            Self::SeasonalMaterials,
            Self::SeasonalPotionRack,
            Self::SeasonalPetInventories,
            Self::SeasonalGiftChests,
            Self::RegularCharacters,
            Self::RegularVault,
            Self::RegularMaterials,
            Self::RegularPotionRack,
            Self::RegularPetInventories,
            Self::RegularGiftChest,
            Self::SeasonalSpoils,
        ]
    }

    /// Whether this section represents seasonal data.
    /// Note: SeasonalSpoils is NOT included because spoils are stored in regular vault.
    pub fn is_seasonal(&self) -> bool {
        matches!(
            self,
            Self::SeasonalCharacters
                | Self::SeasonalVault
                | Self::SeasonalMaterials
                | Self::SeasonalPotionRack
                | Self::SeasonalPetInventories
                | Self::SeasonalGiftChests
        )
    }

    /// Stable string key for settings persistence.
    pub fn to_key(&self) -> &'static str {
        match self {
            Self::SeasonalCharacters => "seasonal_characters",
            Self::SeasonalVault => "seasonal_vault",
            Self::SeasonalMaterials => "seasonal_materials",
            Self::SeasonalPotionRack => "seasonal_potion_rack",
            Self::SeasonalPetInventories => "seasonal_pet_inventories",
            Self::SeasonalGiftChests => "seasonal_gift_chests",
            Self::RegularCharacters => "regular_characters",
            Self::RegularVault => "regular_vault",
            Self::RegularMaterials => "regular_materials",
            Self::RegularPotionRack => "regular_potion_rack",
            Self::RegularPetInventories => "regular_pet_inventories",
            Self::RegularGiftChest => "regular_gift_chest",
            Self::SeasonalSpoils => "seasonal_spoils",
        }
    }

    /// Parse from a settings key string.
    pub fn from_key(s: &str) -> Option<Self> {
        match s {
            "seasonal_characters" => Some(Self::SeasonalCharacters),
            "seasonal_vault" => Some(Self::SeasonalVault),
            "seasonal_materials" => Some(Self::SeasonalMaterials),
            "seasonal_potion_rack" => Some(Self::SeasonalPotionRack),
            "seasonal_pet_inventories" => Some(Self::SeasonalPetInventories),
            "seasonal_gift_chests" => Some(Self::SeasonalGiftChests),
            "regular_characters" => Some(Self::RegularCharacters),
            "regular_vault" => Some(Self::RegularVault),
            "regular_materials" => Some(Self::RegularMaterials),
            "regular_potion_rack" => Some(Self::RegularPotionRack),
            "regular_pet_inventories" => Some(Self::RegularPetInventories),
            "regular_gift_chest" => Some(Self::RegularGiftChest),
            "seasonal_spoils" => Some(Self::SeasonalSpoils),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// TotalEntry
// ---------------------------------------------------------------------------

/// Aggregated count for a single unique item across all storage locations.
#[derive(Debug, Clone)]
pub struct TotalEntry {
    /// Total count of this item (accounts for stack_count on stacked items).
    pub count: u32,
}

impl TotalEntry {
    fn new() -> Self {
        Self { count: 0 }
    }
}

// ---------------------------------------------------------------------------
// TreasuryAggregator
// ---------------------------------------------------------------------------

/// Collects items from all storage sources and produces a flat map of
/// `item_id -> TotalEntry` with aggregated counts.
///
/// Rebuilt only when the underlying data changes (dirty flag).
pub struct TreasuryAggregator {
    /// Aggregated totals: item_id -> entry.
    totals: HashMap<i32, TotalEntry>,
    /// Sorted display order (item_id, count) sorted by count descending.
    sorted: Vec<(i32, u32)>,
}

impl TreasuryAggregator {
    /// Create an empty aggregator.
    pub fn new() -> Self {
        Self {
            totals: HashMap::new(),
            sorted: Vec::new(),
        }
    }

    /// Rebuild the aggregation from scratch.
    ///
    /// Iterates all items in `CharacterCache` (characters + pets) and
    /// `LiveVaultData` (all vault storage types). Empty slots (item_id == -1)
    /// are excluded. Stacked items count by their `stack_count`.
    pub fn rebuild(
        &mut self,
        character_cache: Option<&CharacterCache>,
        live_vault_data: Option<&LiveVaultData>,
    ) {
        self.totals.clear();
        let am = get_asset_manager();

        // --- Characters (equipment, inventory, backpack, extender, belt) ---
        if let Some(cache) = character_cache {
            for character in &cache.characters {
                Self::add_character_items(&mut self.totals, character, am);
            }

            // --- Pet inventories (only pets with inventory unlocked) ---
            for pet in cache
                .regular_pets
                .values()
                .chain(cache.seasonal_pets.values())
            {
                if !pet.has_inventory() {
                    continue;
                }
                for item in &pet.inventory {
                    if item.item_id >= 0 {
                        let (base_id, name_stack) = am.get_stack_info(item.item_id);
                        let stack = if item.stack_count > 1 {
                            item.stack_count as u32
                        } else {
                            name_stack
                        };
                        self.totals
                            .entry(base_id)
                            .or_insert_with(TotalEntry::new)
                            .count += stack;
                    }
                }
            }
        }

        // --- Vault storage (regular + seasonal, all types) ---
        if let Some(data) = live_vault_data {
            for storage in [&data.regular, &data.seasonal] {
                for items in [
                    &storage.vault_items,
                    &storage.material_items,
                    &storage.gift_items,
                    &storage.potion_items,
                    &storage.spoils_items,
                ] {
                    for item in items {
                        if !item.is_empty() {
                            let (base_id, stack_count) = am.get_stack_info(item.item_id);
                            self.totals
                                .entry(base_id)
                                .or_insert_with(TotalEntry::new)
                                .count += stack_count;
                        }
                    }
                }
            }
        }

        // Build sorted display order (descending by count, then by item_id for stability)
        self.sorted = self.totals.iter().map(|(&id, e)| (id, e.count)).collect();
        self.sorted
            .sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    }

    /// Add all non-empty items from a character's slots to the totals map.
    fn add_character_items(
        totals: &mut HashMap<i32, TotalEntry>,
        c: &CachedCharacter,
        am: &realmhound_core::assets::AssetManager,
    ) {
        let slot_groups: [&[CharacterItem]; 5] = [
            &c.equipment,
            &c.inventory,
            &c.backpack,
            &c.backpack_ext,
            &c.belt,
        ];
        for group in &slot_groups {
            for item in *group {
                if !item.is_empty() {
                    let (base_id, name_stack) = am.get_stack_info(item.item_id);
                    let stack = if item.stack_count > 1 {
                        item.stack_count as u32
                    } else {
                        name_stack
                    };
                    totals.entry(base_id).or_insert_with(TotalEntry::new).count += stack;
                }
            }
        }
    }

    /// Get the sorted totals for display (item_id, count), descending by count.
    pub fn sorted_totals(&self) -> &[(i32, u32)] {
        &self.sorted
    }

    /// Whether the aggregator has any data.
    pub fn is_empty(&self) -> bool {
        self.sorted.is_empty()
    }
}

// ---------------------------------------------------------------------------
// TreasurySortMode
// ---------------------------------------------------------------------------

/// How items in the totals header grid are sorted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TreasurySortMode {
    /// Group items by category (default category order), count descending within each group.
    ByCategory,
    /// Group items by category using a user-defined category order, count descending within.
    Custom,
    /// Flat sort by item count descending (no category grouping).
    ByCount,
    /// Smart sort within subcategories: tiered (by tier) > UTs (by fame%) > STs (by fame%).
    Default,
    /// Flat sort by feed power descending.
    ByFeedPower,
    /// Flat sort by fame bonus percentage descending.
    ByFameBonus,
}

impl TreasurySortMode {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::ByCategory => "By Category",
            Self::Custom => "Custom Order",
            Self::ByCount => "By Count",
            Self::Default => "Default (Smart)",
            Self::ByFeedPower => "By Feed Power",
            Self::ByFameBonus => "By Fame %",
        }
    }

    pub fn all() -> &'static [TreasurySortMode] {
        &[
            Self::Default,
            Self::ByCategory,
            Self::Custom,
            Self::ByCount,
            Self::ByFeedPower,
            Self::ByFameBonus,
        ]
    }

    /// Stable string key for settings persistence.
    pub fn to_key(&self) -> &'static str {
        match self {
            Self::ByCategory => "by_category",
            Self::Custom => "custom",
            Self::ByCount => "by_count",
            Self::Default => "default",
            Self::ByFeedPower => "by_feed_power",
            Self::ByFameBonus => "by_fame_bonus",
        }
    }

    /// Parse from a settings key string.
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "by_category" => Some(Self::ByCategory),
            "custom" => Some(Self::Custom),
            "by_count" => Some(Self::ByCount),
            "default" => Some(Self::Default),
            "by_feed_power" => Some(Self::ByFeedPower),
            "by_fame_bonus" => Some(Self::ByFameBonus),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// TreasuryPanel
// ---------------------------------------------------------------------------

/// Item-property filters for the Treasury panel, mirroring the loot-history panel.
///
/// Two orthogonal groups:
///   * Type group (UT / ST / Shiny): item-id based. OR within the group; when no
///     button is enabled the group imposes no constraint. Applied EVERYWHERE
///     (the aggregated totals grid and the bottom sections).
///   * Rarity group (by enchant count: None=0, Uncommon=1, Rare=2, Legendary=3,
///     Divine=4+): per-instance. Applied ONLY to the bottom sections, since the
///     aggregated totals grid has no per-instance enchant data. All-on (the
///     default) and all-off both impose no constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PropFilter {
    ut: bool,
    st: bool,
    shiny: bool,
    r_none: bool,
    r_uncommon: bool,
    r_rare: bool,
    r_legendary: bool,
    r_divine: bool,
}

impl Default for PropFilter {
    fn default() -> Self {
        // Type filters off; all rarity buttons on (= no rarity constraint).
        Self {
            ut: false,
            st: false,
            shiny: false,
            r_none: true,
            r_uncommon: true,
            r_rare: true,
            r_legendary: true,
            r_divine: true,
        }
    }
}

impl PropFilter {
    /// Whether any type filter (UT/ST/Shiny) is enabled.
    fn type_active(&self) -> bool {
        self.ut || self.st || self.shiny
    }

    /// Whether an item passes the type group (UT/ST/Shiny), OR within the group.
    /// Returns true when no type filter is active.
    fn passes_type(&self, am: &AssetManager, item_id: i32) -> bool {
        if !self.type_active() {
            return true;
        }
        (self.ut && am.is_ut(item_id))
            || (self.shiny && am.is_shiny(item_id))
            || (self.st && am.get_object(item_id).map(|o| o.is_st()).unwrap_or(false))
    }

    /// Whether the rarity group narrows results: a strict, non-empty subset is
    /// selected. All-on and all-off both impose no constraint.
    fn rarity_constraining(&self) -> bool {
        let any =
            self.r_none || self.r_uncommon || self.r_rare || self.r_legendary || self.r_divine;
        let all =
            self.r_none && self.r_uncommon && self.r_rare && self.r_legendary && self.r_divine;
        any && !all
    }

    /// Whether an item with `enchant_count` enchants passes the rarity group.
    /// Returns true when the rarity group imposes no constraint.
    fn passes_rarity(&self, enchant_count: usize) -> bool {
        if !self.rarity_constraining() {
            return true;
        }
        match enchant_count {
            0 => self.r_none,
            1 => self.r_uncommon,
            2 => self.r_rare,
            3 => self.r_legendary,
            _ => self.r_divine,
        }
    }

    /// Per-instance match: passes both the type group and the rarity group.
    fn passes(&self, am: &AssetManager, item_id: i32, enchant_count: usize) -> bool {
        self.passes_type(am, item_id) && self.passes_rarity(enchant_count)
    }
}

/// Composite key snapshotting all filter state, used to detect when the display
/// caches need rebuilding. Trailing `Vec<i32>` is the sorted set of active
/// forge-category icon indices.
type TreasuryFilterKey = (
    Vec<i32>,
    Option<String>,
    Option<ItemCategory>,
    Option<ItemSubCategory>,
    PropFilter,
    TreasurySortMode,
    Vec<ItemCategory>,
    Vec<i32>,
);

/// Panel displaying all owned items from all storage locations.
///
/// Follows the same data-forwarding pattern as `VaultPanel`: the app pushes
/// cloned `LiveVaultData` via `set_live_vault_data()` and a cloned
/// `CharacterCache` via `set_character_cache()` whenever either changes.
pub struct TreasuryPanel {
    /// Live vault data (regular + seasonal storage).
    live_vault_data: Option<LiveVaultData>,
    /// Character cache snapshot (characters + pets).
    character_cache: Option<CharacterCache>,
    /// Section display order configured through Settings.
    section_order: Vec<TreasurySection>,
    /// Aggregated item totals across all storage locations.
    aggregator: TreasuryAggregator,
    /// Dirty flag: set when data changes, cleared after aggregator rebuild.
    dirty: bool,
    /// Active item filters: when non-empty, only locations containing ANY of these items are shown (OR logic).
    active_item_filters: HashSet<i32>,
    /// Active text filter: when set, items are filtered by name substring match.
    /// Orthogonal to item filter -- only one is active at a time.
    active_text_filter: Option<String>,
    /// Search bar text input.
    search_text: String,
    /// Previous search text (for change detection).
    search_prev_text: String,
    /// Whether the search suggestion popup is open.
    search_popup_open: bool,
    /// Currently selected suggestion index (keyboard navigation).
    search_selected_index: Option<usize>,
    /// Active forge/tooltip category filters, keyed by `CollectionIcon` frame
    /// index. When non-empty, only items whose category icon is in this set are
    /// shown (OR within the group). Sourced from in-game `collectionIcon` data.
    active_dungeon_filters: HashSet<i32>,
    /// Category search bar text input.
    dungeon_search_text: String,
    /// Previous category search text (for change detection).
    dungeon_search_prev_text: String,
    /// Whether the category search suggestion popup is open.
    dungeon_search_popup_open: bool,
    /// Currently selected category suggestion index (keyboard navigation).
    dungeon_search_selected_index: Option<usize>,
    /// Active category filter.
    active_category: Option<ItemCategory>,
    /// Active sub-category filter (only when a category is selected).
    active_subcategory: Option<ItemSubCategory>,
    /// Item-property filters (UT / ST / Shiny type group + rarity group).
    props: PropFilter,
    /// Whether the totals header is pinned (sticky) at the top.
    totals_pinned: bool,
    /// User-configured height for the totals header area (pixels).
    /// When the header content exceeds this, a vertical scrollbar appears.
    totals_header_height: f32,
    /// Current sort mode for the totals header grid.
    sort_mode: TreasurySortMode,
    /// Custom category order for `TreasurySortMode::Custom`.
    /// Initialized to the default order; user can reorder via drag-and-drop.
    custom_category_order: Vec<ItemCategory>,
    /// Index of the category currently being dragged (Custom sort mode).
    dragging_cat_index: Option<usize>,
    /// Drop target index for category drag-and-drop.
    cat_drop_target: Option<usize>,
    /// Dirty flag: set when sort mode or custom category order changes.
    /// Cleared by the app after persisting to settings.
    sort_settings_dirty: bool,
    /// Cached item categorizer (built lazily from AssetManager).
    categorizer: Option<ItemCategorizer>,

    // --- Per-frame caches (invalidated on data/filter changes) ---
    /// Cached filtered totals for display in the header row.
    cached_display_totals: Vec<(i32, u32)>,
    /// Cached section item counts (indexed by TreasurySection::default_order position).
    cached_section_counts: HashMap<TreasurySection, usize>,
    /// Whether the display caches are valid. Cleared on data or filter change.
    display_cache_valid: bool,
    /// Snapshot of all filter state when cache was last built (for change detection).
    cached_filter_key: TreasuryFilterKey,
    /// Item id clicked in a container this frame (equipped, inventory, pet, or
    /// vault slot). Collected during the immutable render pass and processed
    /// afterwards to toggle the item filter, mirroring a totals-cell click.
    pending_item_click: std::cell::Cell<Option<i32>>,
}

impl TreasuryPanel {
    /// Create a new TreasuryPanel with default section order.
    pub fn new() -> Self {
        let order = TreasurySection::default_order();
        Self {
            live_vault_data: None,
            character_cache: None,
            section_order: order,
            aggregator: TreasuryAggregator::new(),
            dirty: true,
            active_item_filters: HashSet::new(),
            active_text_filter: None,
            search_text: String::new(),
            search_prev_text: String::new(),
            search_popup_open: false,
            search_selected_index: None,
            active_dungeon_filters: HashSet::new(),
            dungeon_search_text: String::new(),
            dungeon_search_prev_text: String::new(),
            dungeon_search_popup_open: false,
            dungeon_search_selected_index: None,
            active_category: None,
            active_subcategory: None,
            props: PropFilter::default(),
            totals_pinned: true,
            totals_header_height: 200.0,
            sort_mode: TreasurySortMode::ByCount,
            custom_category_order: ItemCategory::all().to_vec(),
            dragging_cat_index: None,
            cat_drop_target: None,
            sort_settings_dirty: false,
            categorizer: None,
            cached_display_totals: Vec::new(),
            cached_section_counts: HashMap::new(),
            display_cache_valid: false,
            cached_filter_key: (
                Vec::new(),
                None,
                None,
                None,
                PropFilter::default(),
                TreasurySortMode::ByCount,
                ItemCategory::all().to_vec(),
                Vec::new(),
            ),
            pending_item_click: std::cell::Cell::new(None),
        }
    }

    /// Update the live vault data snapshot.
    ///
    /// Called by the app whenever `LiveVaultData` changes (same 3 call sites
    /// that already forward to `VaultPanel`).
    pub fn set_live_vault_data(&mut self, data: LiveVaultData) {
        self.live_vault_data = Some(data);
        self.dirty = true;
    }

    /// Update the character cache snapshot.
    ///
    /// Called by the app whenever the character cache is modified (character
    /// list refresh, equipment update, etc.).
    pub fn set_character_cache(&mut self, cache: CharacterCache) {
        self.character_cache = Some(cache);
        self.dirty = true;
    }

    /// Get the current section display order (for settings persistence).
    pub fn section_order(&self) -> &[TreasurySection] {
        &self.section_order
    }

    /// Swap two sections by index (for settings reorder UI).
    pub fn swap_sections(&mut self, a: usize, b: usize) {
        if a < self.section_order.len() && b < self.section_order.len() {
            self.section_order.swap(a, b);
        }
    }

    /// Reset section order to the design document defaults.
    pub fn reset_section_order(&mut self) {
        self.section_order = TreasurySection::default_order();
    }

    /// Apply section order from persisted settings keys.
    ///
    /// Parses the key strings, preserves the persisted order, and appends any
    /// sections that are missing (e.g. newly added) at the end.
    pub fn apply_section_order_from_keys(&mut self, keys: &[String]) {
        if keys.is_empty() {
            return; // keep default order
        }
        let mut used = std::collections::HashSet::new();
        let mut order: Vec<TreasurySection> = Vec::new();
        for key in keys {
            if let Some(section) = TreasurySection::from_key(key) {
                if used.insert(section) {
                    order.push(section);
                }
            }
        }
        // Append any missing sections (forward-compatibility)
        for section in TreasurySection::default_order() {
            if used.insert(section) {
                order.push(section);
            }
        }
        self.section_order = order;
    }

    /// Get the current sort mode.
    pub fn sort_mode(&self) -> TreasurySortMode {
        self.sort_mode
    }

    /// Set the sort mode (called during settings load).
    pub fn set_sort_mode(&mut self, mode: TreasurySortMode) {
        self.sort_mode = mode;
    }

    /// Get the current custom category order.
    pub fn custom_category_order(&self) -> &[ItemCategory] {
        &self.custom_category_order
    }

    /// Apply custom category order from persisted settings keys.
    ///
    /// Parses the key strings, preserves the persisted order, and appends any
    /// categories that are missing at the end.
    pub fn apply_custom_category_order_from_keys(&mut self, keys: &[String]) {
        if keys.is_empty() {
            return; // keep default order
        }
        let mut used = std::collections::HashSet::new();
        let mut order: Vec<ItemCategory> = Vec::new();
        for key in keys {
            if let Some(cat) = ItemCategory::from_key(key) {
                if used.insert(cat) {
                    order.push(cat);
                }
            }
        }
        // Append any missing categories (forward-compatibility)
        for &cat in ItemCategory::all() {
            if used.insert(cat) {
                order.push(cat);
            }
        }
        self.custom_category_order = order;
    }

    /// Check and clear the sort-settings-dirty flag.
    ///
    /// Returns `true` if sort mode or custom category order changed since last call.
    /// The app should call this after rendering and persist settings if `true`.
    pub fn take_sort_settings_dirty(&mut self) -> bool {
        let was_dirty = self.sort_settings_dirty;
        self.sort_settings_dirty = false;
        was_dirty
    }

    // -----------------------------------------------------------------------
    // Rendering
    // -----------------------------------------------------------------------

    /// Current composite filter key for cache change detection.
    fn filter_key(&self) -> TreasuryFilterKey {
        let mut filters: Vec<i32> = self.active_item_filters.iter().copied().collect();
        filters.sort(); // Stable ordering for comparison
        let mut dungeon_filters: Vec<i32> = self.active_dungeon_filters.iter().copied().collect();
        dungeon_filters.sort();
        (
            filters,
            self.active_text_filter.clone(),
            self.active_category,
            self.active_subcategory,
            self.props,
            self.sort_mode,
            self.custom_category_order.clone(),
            dungeon_filters,
        )
    }

    /// Rebuild the aggregator if data has changed since last render.
    fn rebuild_if_dirty(&mut self) {
        // Build the categorizer lazily from the global asset manager.
        // Must happen before cache rebuild so category-based sorting works.
        if self.categorizer.is_none() {
            if let Some(cz) = get_asset_manager().build_categorizer() {
                self.categorizer = Some(cz);
                // Categorizer just became available -- invalidate so caches
                // are rebuilt with correct category ranks.
                self.display_cache_valid = false;
            }
        }

        if self.dirty {
            self.aggregator
                .rebuild(self.character_cache.as_ref(), self.live_vault_data.as_ref());
            self.dirty = false;
            self.display_cache_valid = false;
        }

        // Invalidate display cache when filters change
        if self.filter_key() != self.cached_filter_key {
            self.display_cache_valid = false;
        }

        // Rebuild display caches if invalid
        if !self.display_cache_valid {
            self.rebuild_display_caches();
        }
    }

    /// Rebuild the cached display totals and section counts.
    ///
    /// Called only when data or filters change, not every frame.
    /// Stackable item variants are combined into one entry keyed by their
    /// canonical base_id with summed counts.
    fn rebuild_display_caches(&mut self) {
        // Cache filter state
        self.cached_filter_key = self.filter_key();

        // Rebuild filtered display totals, combining stackable variants
        // NOTE: active_item_filter is intentionally NOT used here
        let text_filter = self.active_text_filter.as_ref().map(|s| s.to_lowercase());
        let cat_filter = self.active_category;
        let subcat_filter = self.active_subcategory;
        let props = self.props;
        let categorizer = self.categorizer.as_ref();
        let am = get_asset_manager();

        // Combine stackable variants by base_id
        let mut combined: HashMap<i32, u32> = HashMap::new();
        for &(id, count) in self.aggregator.sorted_totals() {
            // NOTE: active_item_filter is NOT applied here.
            // Treasury always shows all items; the filter only affects search results below.
            // The clicked item gets a visual highlight via render_totals_cell().

            // Apply text filter (substring match on item name)
            if let Some(ref text) = text_filter {
                let name = am.object_name(id).unwrap_or_default().to_lowercase();
                if !name.contains(text.as_str()) {
                    continue;
                }
            }
            // Apply type filter (UT/ST/Shiny, item-id based). Rarity is intentionally
            // NOT applied to the aggregated grid -- it has no per-instance enchant data.
            if !props.passes_type(am, id) {
                continue;
            }
            // Apply forge/tooltip category filter (in-game collectionIcon, OR group)
            if !self.active_dungeon_filters.is_empty() {
                match am.item_category_icon(id) {
                    Some(icon) if self.active_dungeon_filters.contains(&icon) => {}
                    _ => continue,
                }
            }
            // Apply category filter
            if let Some(cat) = cat_filter {
                if let Some(ref cz) = categorizer {
                    let (item_cat, item_sub) = cz.get(id);
                    if item_cat != cat {
                        continue;
                    }
                    if let Some(sub) = subcat_filter {
                        if item_sub != sub {
                            continue;
                        }
                    }
                }
            }
            let (base_id, _) = am.get_stack_info(id);
            *combined.entry(base_id).or_insert(0) += count;
        }

        self.cached_display_totals = combined.into_iter().collect();

        // Sort based on active sort mode
        match self.sort_mode {
            TreasurySortMode::ByCount => {
                // Flat sort: count descending, then item_id for stability
                self.cached_display_totals
                    .sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            }
            TreasurySortMode::ByFeedPower => {
                // Flat sort by feed power descending
                self.cached_display_totals.sort_by(|a, b| {
                    let fp_a = am.get_object(a.0).map(|o| o.feed_power).unwrap_or(0);
                    let fp_b = am.get_object(b.0).map(|o| o.feed_power).unwrap_or(0);
                    fp_b.cmp(&fp_a).then(b.1.cmp(&a.1)).then(a.0.cmp(&b.0))
                });
            }
            TreasurySortMode::ByFameBonus => {
                // Flat sort by fame bonus descending
                self.cached_display_totals.sort_by(|a, b| {
                    let fb_a = am.get_object(a.0).map(|o| o.fame_bonus).unwrap_or(0);
                    let fb_b = am.get_object(b.0).map(|o| o.fame_bonus).unwrap_or(0);
                    fb_b.cmp(&fb_a).then(b.1.cmp(&a.1)).then(a.0.cmp(&b.0))
                });
            }
            TreasurySortMode::Default => {
                // Smart sort: group by category, then within each subcategory:
                // 1. Tiered items first (by tier ascending: T0 < T1 < ... < T14)
                // 2. UTs next (by fame bonus descending)
                // 3. STs last (by fame bonus descending)
                let order = ItemCategory::all();
                let cat_rank: HashMap<ItemCategory, usize> =
                    order.iter().enumerate().map(|(i, &c)| (c, i)).collect();

                self.cached_display_totals.sort_by(|a, b| {
                    let obj_a = am.get_object(a.0);
                    let obj_b = am.get_object(b.0);

                    // Get categories
                    let (cat_a, sub_a) = categorizer
                        .as_ref()
                        .map(|cz| cz.get(a.0))
                        .unwrap_or((ItemCategory::Other, ItemSubCategory::Uncategorized));
                    let (cat_b, sub_b) = categorizer
                        .as_ref()
                        .map(|cz| cz.get(b.0))
                        .unwrap_or((ItemCategory::Other, ItemSubCategory::Uncategorized));

                    // Primary: category order
                    let cat_ord = cat_rank
                        .get(&cat_a)
                        .unwrap_or(&usize::MAX)
                        .cmp(cat_rank.get(&cat_b).unwrap_or(&usize::MAX));
                    if cat_ord != std::cmp::Ordering::Equal {
                        return cat_ord;
                    }

                    // Secondary: subcategory order
                    let sub_ord = (sub_a as usize).cmp(&(sub_b as usize));
                    if sub_ord != std::cmp::Ordering::Equal {
                        return sub_ord;
                    }

                    // Within same subcategory: tiered > UT > ST
                    // Rarity rank: 0 = tiered, 1 = UT, 2 = ST, 3 = other
                    let rarity_rank = |obj: &Option<ObjectAsset>| -> i32 {
                        match obj {
                            Some(o) if o.is_tiered() => 0,
                            Some(o) if o.is_ut() => 1,
                            Some(o) if o.is_st() => 2,
                            _ => 3,
                        }
                    };
                    let rank_a = rarity_rank(&obj_a);
                    let rank_b = rarity_rank(&obj_b);
                    let rank_ord = rank_a.cmp(&rank_b);
                    if rank_ord != std::cmp::Ordering::Equal {
                        return rank_ord;
                    }

                    // Within same rarity:
                    // - Tiered: by tier ascending (T0 < T14)
                    // - UT/ST: by fame bonus descending
                    if rank_a == 0 {
                        // Tiered: sort by tier ascending
                        let tier_a = obj_a.as_ref().and_then(|o| o.get_tier()).unwrap_or(99);
                        let tier_b = obj_b.as_ref().and_then(|o| o.get_tier()).unwrap_or(99);
                        let tier_ord = tier_a.cmp(&tier_b);
                        if tier_ord != std::cmp::Ordering::Equal {
                            return tier_ord;
                        }
                    } else {
                        // UT/ST: sort by fame bonus descending (high fame first)
                        let fb_a = obj_a.as_ref().map(|o| o.fame_bonus).unwrap_or(0);
                        let fb_b = obj_b.as_ref().map(|o| o.fame_bonus).unwrap_or(0);
                        let fb_ord = fb_b.cmp(&fb_a);
                        if fb_ord != std::cmp::Ordering::Equal {
                            return fb_ord;
                        }
                        // Secondary: feed power descending
                        let fp_a = obj_a.as_ref().map(|o| o.feed_power).unwrap_or(0);
                        let fp_b = obj_b.as_ref().map(|o| o.feed_power).unwrap_or(0);
                        let fp_ord = fp_b.cmp(&fp_a);
                        if fp_ord != std::cmp::Ordering::Equal {
                            return fp_ord;
                        }
                    }

                    // Fallback: count descending, then item_id for stability
                    b.1.cmp(&a.1).then(a.0.cmp(&b.0))
                });
            }
            TreasurySortMode::ByCategory | TreasurySortMode::Custom => {
                // Group by category in the chosen order, count descending within each group
                let order: &[ItemCategory] = match self.sort_mode {
                    TreasurySortMode::Custom => &self.custom_category_order,
                    _ => ItemCategory::all(),
                };
                // Build a lookup: category -> position index
                let cat_rank: HashMap<ItemCategory, usize> =
                    order.iter().enumerate().map(|(i, &c)| (c, i)).collect();
                let cat_for_item = |item_id: i32| -> usize {
                    if let Some(ref cz) = categorizer {
                        let (cat, _) = cz.get(item_id);
                        *cat_rank.get(&cat).unwrap_or(&usize::MAX)
                    } else {
                        usize::MAX
                    }
                };
                self.cached_display_totals.sort_by(|a, b| {
                    cat_for_item(a.0)
                        .cmp(&cat_for_item(b.0))
                        .then(b.1.cmp(&a.1))
                        .then(a.0.cmp(&b.0))
                });
            }
        }

        // Rebuild section counts
        self.cached_section_counts.clear();
        for &section in &TreasurySection::default_order() {
            let count = if self.active_item_filters.is_empty() {
                self.section_item_count(section)
            } else {
                // Multi-filter: sum counts for each filter (items may overlap, but that's fine for display)
                self.active_item_filters
                    .iter()
                    .map(|&filter_id| self.section_filter_match_count(section, filter_id))
                    .sum()
            };
            self.cached_section_counts.insert(section, count);
        }

        self.display_cache_valid = true;
    }

    /// Render the full Treasury panel: sticky header + scrollable body.
    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext) {
        realmhound_core::prof_function!();
        // Rebuild aggregation if data changed
        self.rebuild_if_dirty();

        let shadcn = ctx.shadcn;

        // --- Filter toolbar (title + search + category bar): single band ---
        shadcn.header_band(ui, shadcn.secondary_header_fill(), |ui| {
            self.render_filter_toolbar(ui, ctx.sprite_renderer, shadcn);
        });

        if self.totals_pinned {
            // --- Item grid with constrained max height + internal scroll ---
            ScrollArea::vertical()
                .id_salt("totals_header_scroll")
                .max_height(self.totals_header_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    self.render_item_grid(ui, ctx.sprite_renderer);
                });

            // --- Drag handle for resizing the header area ---
            self.render_resize_handle(ui);
            shadcn.separator(ui);

            // --- Scrollable body: all sections ---
            let nav = super::scroll_nav::ScrollNav::read(ui, "treasury_body_scroll");
            let mut body_area = ScrollArea::vertical()
                .id_salt("totals_body_scroll")
                .auto_shrink([false, false]);
            body_area = nav.apply(body_area);
            let body_out = body_area.show(ui, |ui| {
                self.render_body(ui, ctx.sprite_renderer);
            });
            nav.store(ui, &body_out);
        } else {
            // Everything scrolls together when unpinned
            let nav = super::scroll_nav::ScrollNav::read(ui, "treasury_body_scroll");
            let mut combined_area = ScrollArea::vertical().auto_shrink([false, false]);
            combined_area = nav.apply(combined_area);
            let combined_out = combined_area.show(ui, |ui| {
                self.render_item_grid(ui, ctx.sprite_renderer);
                shadcn.separator(ui);
                self.render_body(ui, ctx.sprite_renderer);
            });
            nav.store(ui, &combined_out);
        }
    }

    /// Render a horizontal drag handle for resizing the totals header height.
    fn render_resize_handle(&mut self, ui: &mut egui::Ui) {
        let handle_height = 6.0;
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), handle_height),
            egui::Sense::click_and_drag(),
        );

        // Visual feedback
        let color = if response.dragged() || response.hovered() {
            Color32::from_rgb(120, 140, 255)
        } else {
            Color32::from_rgb(70, 70, 80)
        };

        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            let center_y = rect.center().y;
            // Draw three horizontal dots/lines as grip indicator
            for &dy in &[-1.0_f32, 1.0] {
                painter.line_segment(
                    [
                        egui::pos2(rect.center().x - 20.0, center_y + dy),
                        egui::pos2(rect.center().x + 20.0, center_y + dy),
                    ],
                    egui::Stroke::new(1.0_f32, color),
                );
            }
        }

        // Change cursor to resize
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }

        // Apply drag delta
        if response.dragged() {
            let delta = response.drag_delta().y;
            self.totals_header_height = (self.totals_header_height + delta).clamp(60.0, 800.0);
        }
    }

    /// Render the scrollable body sections.
    fn render_body(&mut self, ui: &mut egui::Ui, sprite_renderer: &mut SpriteRenderer) {
        realmhound_core::prof_function!();
        let has_chars = self.character_cache.is_some();
        let has_vault = self.live_vault_data.is_some();

        if !has_chars && !has_vault {
            crate::panels::empty_state(
                ui,
                "No data loaded yet.",
                Some("Capture vault data or fetch characters to populate."),
            );
            return;
        }

        // Render each section in display order (index-based to avoid clone)
        let section_count = self.section_order.len();
        for i in 0..section_count {
            let section = self.section_order[i];
            self.render_section(ui, section, sprite_renderer);
        }

        // Handle a click on any container item (equipped, inventory, pet, or
        // vault slot): toggle it in the item filter, same as a totals cell.
        if let Some(raw_id) = self.pending_item_click.take() {
            // Normalize stackable variants (e.g. Tarot Card x5/x10) to their base
            // id so the filter matches every stack size, like the search bar.
            let (item_id, _) = get_asset_manager().get_stack_info(raw_id);
            if self.active_item_filters.contains(&item_id) {
                self.active_item_filters.remove(&item_id);
            } else {
                self.active_item_filters.insert(item_id);
            }
            self.active_text_filter = None;
            self.search_text.clear();
            self.search_prev_text.clear();
            self.search_popup_open = false;
            self.search_selected_index = None;
        }
    }

    // -----------------------------------------------------------------------
    // Sticky totals header
    // -----------------------------------------------------------------------

    /// Render the filter toolbar: title row + search bar + category filters.
    /// This is always visible (stays at top), never scrolls away.
    fn render_filter_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        shadcn: &Shadcn,
    ) {
        // A wrapping flow (not a fixed-height band row) so trailing category
        // pills fill the leftover space on the toolbar line first and only wrap
        // onto new lines once that line is full.
        ui.horizontal_wrapped(|ui| {
            ui.set_min_height(Shadcn::BAND_ROW_HEIGHT);
            ui.spacing_mut().item_spacing.y = 4.0;

            // Unique item count acts as the row's header (in place of the old "Treasury" label).
            if self.aggregator.is_empty() {
                ui.label(RichText::new("No items").color(Color32::GRAY).italics());
            } else {
                let unique_count = self.aggregator.sorted_totals().len();
                ui.label(RichText::new(format!("{} unique items", unique_count)).strong());
            }

            // Pin toggle
            let pin_icon = if self.totals_pinned {
                "\u{1F4CC}"
            } else {
                "\u{1F4CB}"
            };
            let pin_tooltip = if self.totals_pinned {
                "Unpin totals header (scroll with content)"
            } else {
                "Pin totals header (stay at top)"
            };
            if shadcn.btn(ui, pin_icon).hover_tip(pin_tooltip).clicked() {
                self.totals_pinned = !self.totals_pinned;
            }

            ui.separator();

            // Search bar on the same row
            if !self.aggregator.is_empty() {
                self.render_search_bar(ui, sprite_renderer, shadcn);
                ui.separator();
                self.render_dungeon_search_bar(ui, sprite_renderer, shadcn);
            }

            // Expand All / Collapse All for section headers
            ui.separator();
            let sections = TreasurySection::default_order();
            let all_expanded = sections.iter().all(|&s| {
                let id = egui::Id::new(("treasury_section", s.to_key()));
                CollapsingState::load_with_default_open(ui.ctx(), id, true).is_open()
            });
            let label = if all_expanded {
                "⏶ Collapse All"
            } else {
                "⏷ Expand All"
            };
            let hover = if all_expanded {
                "Fold every section"
            } else {
                "Unfold every section"
            };
            if shadcn.btn(ui, label).hover_tip(hover).clicked() {
                for &s in &sections {
                    let id = egui::Id::new(("treasury_section", s.to_key()));
                    let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, true);
                    state.set_open(!all_expanded);
                    state.store(ui.ctx());
                }
            }

            // Filter pills inline with search bar (same row)
            if !self.active_item_filters.is_empty() {
                ui.separator();

                // Collect filter IDs to avoid borrow issues
                let filter_ids: Vec<i32> = self.active_item_filters.iter().copied().collect();
                let mut removed_id: Option<i32> = None;

                for &item_id in &filter_ids {
                    let item_name = sprite_renderer
                        .item_name(item_id)
                        .unwrap_or_else(|| format!("Item 0x{:04X}", item_id));

                    let bg_color = Color32::from_rgb(60, 50, 70);
                    if sprite_renderer.render_filter_pill(ui, item_id, &item_name, bg_color) {
                        removed_id = Some(item_id);
                    }
                }

                // Remove clicked filter
                if let Some(id) = removed_id {
                    self.active_item_filters.remove(&id);
                }

                // Clear all button
                if shadcn
                    .btn(ui, "Clear")
                    .hover_tip("Clear all item filters")
                    .clicked()
                {
                    self.active_item_filters.clear();
                }
            }

            // Selected forge-category pills continue in this same wrapping flow,
            // filling the toolbar line's leftover space before wrapping.
            self.render_dungeon_pills(ui, sprite_renderer, shadcn);
        });

        shadcn.full_width_separator(ui);

        // Category / shiny filter bar: matrix categories, property/rarity filters,
        // and sort order -- sits right above the item grid.
        self.render_category_bar(ui, sprite_renderer, shadcn);
    }

    /// Render the item grid (the totals cells). This part scrolls when pinned.
    fn render_item_grid(&mut self, ui: &mut egui::Ui, sprite_renderer: &mut SpriteRenderer) {
        realmhound_core::prof_function!();
        if !self.aggregator.is_empty() {
            // Use cached display totals (rebuilt only when data/filters change)
            // Wrapping item cells (no horizontal scroll -- items wrap to next row)
            let display_count = self.cached_display_totals.len();

            // Clone filters to avoid borrow issues when mutating after click
            let active_filters_snapshot = self.active_item_filters.clone();
            let mut clicked_item: Option<i32> = None;

            ui.horizontal_wrapped(|ui: &mut egui::Ui| {
                ui.spacing_mut().item_spacing = egui::vec2(5.0, 5.0);
                for idx in 0..display_count {
                    let (item_id, count) = self.cached_display_totals[idx];
                    if Self::render_totals_cell(
                        ui,
                        item_id,
                        count,
                        &active_filters_snapshot,
                        sprite_renderer,
                    ) {
                        clicked_item = Some(item_id);
                    }
                }
            });

            // Handle click outside the closure to avoid borrow conflicts
            if let Some(item_id) = clicked_item {
                // Toggle filter on click: add if not present, remove if present (multi-select OR)
                if self.active_item_filters.contains(&item_id) {
                    self.active_item_filters.remove(&item_id);
                } else {
                    self.active_item_filters.insert(item_id);
                }
                // Clear text filter when using item filters
                self.active_text_filter = None;
                self.search_text.clear();
                self.search_prev_text.clear();
                self.search_popup_open = false;
                self.search_selected_index = None;
            }
        }
    }

    /// Render a single totals cell: item sprite with count overlay (bottom-left).
    /// Returns true if the cell was clicked.
    fn render_totals_cell(
        ui: &mut egui::Ui,
        item_id: i32,
        count: u32,
        active_filters: &HashSet<i32>,
        sprite_renderer: &mut SpriteRenderer,
    ) -> bool {
        realmhound_core::prof_scope!("totals_cell");
        const CELL_SIZE: f32 = 32.0;
        // Total cell width = sprite only (no padding for compact layout)
        const CELL_OUTER: f32 = CELL_SIZE;
        let is_active = active_filters.contains(&item_id);

        let mut clicked = false;

        // allocate_ui reserves a fixed width so horizontal_wrapped can measure
        // and break to the next row correctly
        ui.allocate_ui(egui::vec2(CELL_OUTER, CELL_OUTER), |ui| {
            ui.push_id(item_id, |ui| {
                // Highlight frame when this cell is the active filter
                let frame = if is_active {
                    egui::Frame::NONE
                        .fill(Color32::from_rgb(60, 60, 80))
                        .stroke(egui::Stroke::new(2.0_f32, Color32::from_rgb(120, 140, 255)))
                        .corner_radius(2.0)
                } else {
                    egui::Frame::NONE
                        .fill(crate::ui_colors::slot_fill(ui.visuals()))
                        .stroke(egui::Stroke::new(
                            1.0_f32,
                            crate::ui_colors::slot_stroke(ui.visuals()),
                        ))
                        .corner_radius(2.0)
                };

                let frame_resp = frame.show(ui, |ui| {
                    ui.set_min_width(CELL_SIZE);
                    ui.set_max_width(CELL_SIZE);

                    // Clickable item sprite at 32x32
                    let response = {
                        realmhound_core::prof_scope!("totals_cell_sprite");
                        sprite_renderer.render_item_tile_clickable_sized(
                            ui,
                            item_id,
                            None,
                            &[],
                            CELL_SIZE,
                        )
                    };
                    if response.clicked() {
                        clicked = true;
                    }
                    let rect = response.rect;
                    rect
                });

                // Draw count overlay on bottom-right of the sprite rect
                let sprite_rect = frame_resp.inner;
                let count_text = format!("x{}", count);
                let font = egui::FontId::proportional(12.0);
                let text_pos = egui::pos2(sprite_rect.right() - 1.0, sprite_rect.bottom() - 1.0);
                let text_color = if is_active {
                    Color32::from_rgb(120, 140, 255)
                } else {
                    Color32::WHITE
                };

                // Shadow outline for readability (8 offsets for thicker outline)
                let painter = ui.painter();
                for &(dx, dy) in &[
                    (1.0_f32, 0.0_f32),
                    (-1.0, 0.0),
                    (0.0, 1.0),
                    (0.0, -1.0),
                    (1.0, 1.0),
                    (-1.0, -1.0),
                    (1.0, -1.0),
                    (-1.0, 1.0),
                ] {
                    painter.text(
                        egui::pos2(text_pos.x + dx, text_pos.y + dy),
                        egui::Align2::RIGHT_BOTTOM,
                        &count_text,
                        font.clone(),
                        Color32::BLACK,
                    );
                }
                painter.text(
                    text_pos,
                    egui::Align2::RIGHT_BOTTOM,
                    &count_text,
                    font,
                    text_color,
                );
            });
        });

        clicked
    }

    /// Render the search bar with autocomplete suggestions.
    ///
    /// Searches owned items (those in the aggregator) by display name. Selecting
    /// a suggestion adds the item to `active_item_filters` (multi-select OR filter).
    /// Typing without selecting applies a live text filter (substring match).
    fn render_search_bar(
        &mut self,
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        shadcn: &Shadcn,
    ) {
        // Reset selection when search text changes
        if self.search_text != self.search_prev_text {
            self.search_selected_index = None;
            self.search_prev_text = self.search_text.clone();
            // When text changes, clear item filters and apply text filter instead
            self.active_item_filters.clear();
            if self.search_text.is_empty() {
                self.active_text_filter = None;
            } else {
                self.active_text_filter = Some(self.search_text.clone());
            }
        }

        {
            ui.label("🔍");
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.search_text)
                    .desired_width(180.0)
                    .hint_text("Search items..."),
            );

            // Enter pressed (TextEdit loses focus on Enter)
            let enter_pressed =
                response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

            // Open popup when focused and has text
            if response.has_focus() && !self.search_text.is_empty() {
                self.search_popup_open = true;
            }

            // Clear button
            if !self.search_text.is_empty() && shadcn.btn(ui, "\u{2715}").clicked() {
                self.search_text.clear();
                self.search_popup_open = false;
                self.search_selected_index = None;
                self.active_item_filters.clear();
                self.active_text_filter = None;
            }

            // Build search results: match item names in the aggregator.
            // Stackable variants (e.g. "Schematic x10", "Schematic x50") are
            // merged into a single entry using the base name and combined count.
            let search_lower = self.search_text.to_lowercase();
            let mut results: Vec<(i32, String, u32)> = Vec::new();

            if !self.search_text.is_empty() {
                let am = get_asset_manager();
                // Track which base_ids we've already added
                let mut seen_bases: std::collections::HashSet<i32> =
                    std::collections::HashSet::new();

                for &(item_id, count) in self.aggregator.sorted_totals() {
                    if let Some(name) = sprite_renderer.item_name(item_id) {
                        let (base_id, _stack) = am.get_stack_info(item_id);

                        // Strip " xN" suffix for display in autocomplete
                        let display_name = match AssetManager::parse_stack_suffix(&name) {
                            Some((base, _)) => base.to_string(),
                            None => name,
                        };

                        if !display_name.to_lowercase().contains(&search_lower) {
                            continue;
                        }

                        if seen_bases.contains(&base_id) {
                            // Merge count into existing entry
                            if let Some(entry) =
                                results.iter_mut().find(|(id, _, _)| *id == base_id)
                            {
                                entry.2 += count;
                            }
                        } else {
                            seen_bases.insert(base_id);
                            results.push((base_id, display_name, count));
                        }
                    }
                    if results.len() >= 15 {
                        break;
                    }
                }
            }

            let total_results = results.len();

            // Handle Enter key: apply selected or first result
            if enter_pressed && self.search_popup_open && total_results > 0 {
                let idx = self.search_selected_index.unwrap_or(0);
                if let Some((base_id, name, _)) = results.get(idx) {
                    self.active_item_filters.insert(*base_id);
                    self.active_text_filter = None;
                    self.search_text = name.clone();
                    self.search_prev_text = self.search_text.clone();
                    self.search_popup_open = false;
                    self.search_selected_index = None;
                }
            }

            // Handle keyboard navigation when popup is open
            if self.search_popup_open && response.has_focus() && total_results > 0 {
                let key_down = ui.input(|i| i.key_pressed(egui::Key::ArrowDown));
                let key_up = ui.input(|i| i.key_pressed(egui::Key::ArrowUp));
                let key_escape = ui.input(|i| i.key_pressed(egui::Key::Escape));

                if key_escape {
                    self.search_popup_open = false;
                    self.search_selected_index = None;
                } else if key_down {
                    self.search_selected_index = Some(match self.search_selected_index {
                        None => 0,
                        Some(i) => (i + 1).min(total_results - 1),
                    });
                } else if key_up {
                    self.search_selected_index = match self.search_selected_index {
                        None => None,
                        Some(0) => None,
                        Some(i) => Some(i - 1),
                    };
                }
            }

            // Render autocomplete popup below the text input
            if self.search_popup_open && !self.search_text.is_empty() && !results.is_empty() {
                let popup_id = ui.make_persistent_id("totals_search_popup");
                let popup_pos = response.rect.left_bottom() + egui::vec2(0.0, 2.0);

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
                                    for (idx, (item_id, name, count)) in results.iter().enumerate()
                                    {
                                        let is_selected = self.search_selected_index == Some(idx);

                                        ui.horizontal(|ui| {
                                            // Small item sprite (match Loot History dropdown)
                                            let (rect, _) = ui.allocate_exact_size(
                                                egui::vec2(20.0, 20.0),
                                                egui::Sense::hover(),
                                            );
                                            if *item_id > 0 {
                                                if !sprite_renderer.draw_outlined_sprite_in_rect(
                                                    ui, *item_id, rect,
                                                ) {
                                                    sprite_renderer
                                                        .draw_sprite_in_rect(ui, *item_id, rect);
                                                }
                                            }

                                            let label_text = format!("{} (x{})", name, count);
                                            let label_response = ui.add(
                                                egui::Button::new(&label_text)
                                                    .selected(is_selected),
                                            );

                                            if is_selected {
                                                label_response
                                                    .scroll_to_me(Some(egui::Align::Center));
                                            }

                                            if label_response.clicked() {
                                                self.active_item_filters.insert(*item_id);
                                                self.active_text_filter = None;
                                                self.search_text = name.clone();
                                                self.search_prev_text = self.search_text.clone();
                                                self.search_popup_open = false;
                                                self.search_selected_index = None;
                                            }
                                        });
                                    }
                                });
                        });
                    });

                // Close popup when clicking outside
                if ui.input(|i| i.pointer.any_click()) && !response.has_focus() {
                    self.search_popup_open = false;
                    self.search_selected_index = None;
                }
            }
        }
    }

    /// Render the forge/tooltip category filter row: a dedicated search bar with
    /// icon suggestions plus removable pills for the selected categories.
    ///
    /// Categories are the in-game forge groupings (shared `collectionIcon`), so
    /// several dungeons can share one category (e.g. Oryx's Castle + Wine
    /// Cellar). Selection is multi-select OR: an item matches if its category is
    /// any selected one.
    fn render_dungeon_search_bar(
        &mut self,
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        shadcn: &Shadcn,
    ) {
        let categories = get_asset_manager().dungeon_categories();
        if categories.is_empty() {
            return;
        }

        // Reset selection when search text changes.
        if self.dungeon_search_text != self.dungeon_search_prev_text {
            self.dungeon_search_selected_index = None;
            self.dungeon_search_prev_text = self.dungeon_search_text.clone();
        }

        let response = ui.add(
            egui::TextEdit::singleline(&mut self.dungeon_search_text)
                .desired_width(220.0)
                .hint_text("Filter UT/blueprint by dungeon..."),
        );

        let enter_pressed = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

        if response.has_focus() {
            self.dungeon_search_popup_open = true;
        }

        if !self.dungeon_search_text.is_empty() && shadcn.btn(ui, "\u{2715}").clicked() {
            self.dungeon_search_text.clear();
            self.dungeon_search_popup_open = false;
            self.dungeon_search_selected_index = None;
        }

        // Build suggestions (categories matching the query, not already selected).
        let query_lower = self.dungeon_search_text.to_lowercase();
        let results: Vec<usize> = categories
            .iter()
            .enumerate()
            .filter(|(_, cat)| {
                !self.active_dungeon_filters.contains(&cat.icon_index)
                    && cat.matches_query(&query_lower)
            })
            .map(|(i, _)| i)
            .collect();
        let total_results = results.len();

        // Enter: apply selected (or first) suggestion.
        if enter_pressed && self.dungeon_search_popup_open && total_results > 0 {
            let idx = self
                .dungeon_search_selected_index
                .unwrap_or(0)
                .min(total_results - 1);
            if let Some(&cat_idx) = results.get(idx) {
                self.active_dungeon_filters
                    .insert(categories[cat_idx].icon_index);
                self.dungeon_search_text.clear();
                self.dungeon_search_prev_text.clear();
                self.dungeon_search_selected_index = None;
                self.dungeon_search_popup_open = false;
            }
        }

        // Keyboard navigation.
        if self.dungeon_search_popup_open && response.has_focus() && total_results > 0 {
            let key_down = ui.input(|i| i.key_pressed(egui::Key::ArrowDown));
            let key_up = ui.input(|i| i.key_pressed(egui::Key::ArrowUp));
            let key_escape = ui.input(|i| i.key_pressed(egui::Key::Escape));

            if key_escape {
                self.dungeon_search_popup_open = false;
                self.dungeon_search_selected_index = None;
            } else if key_down {
                self.dungeon_search_selected_index =
                    Some(match self.dungeon_search_selected_index {
                        None => 0,
                        Some(i) => (i + 1).min(total_results - 1),
                    });
            } else if key_up {
                self.dungeon_search_selected_index = match self.dungeon_search_selected_index {
                    None => None,
                    Some(0) => None,
                    Some(i) => Some(i - 1),
                };
            }
        }

        // Suggestion popup.
        if self.dungeon_search_popup_open && !results.is_empty() {
            let popup_id = ui.make_persistent_id("dungeon_category_popup");
            let popup_pos = response.rect.left_bottom() + egui::vec2(0.0, 2.0);
            let mut clicked_icon: Option<i32> = None;

            egui::Area::new(popup_id)
                .fixed_pos(popup_pos)
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_min_width(220.0);
                        ui.set_max_height(320.0);
                        egui::ScrollArea::vertical()
                            .max_height(300.0)
                            .show(ui, |ui| {
                                for (row, &cat_idx) in results.iter().enumerate() {
                                    let cat = &categories[cat_idx];
                                    let is_selected =
                                        self.dungeon_search_selected_index == Some(row);
                                    ui.horizontal(|ui| {
                                        let (rect, _) = ui.allocate_exact_size(
                                            egui::vec2(20.0, 20.0),
                                            egui::Sense::hover(),
                                        );
                                        sprite_renderer.draw_collection_icon(
                                            ui,
                                            cat.icon_index,
                                            rect,
                                        );

                                        let label_text = format!(
                                            "{} ({})",
                                            cat.display_name,
                                            cat.item_ids.len()
                                        );
                                        let resp = ui.add(
                                            egui::Button::new(&label_text).selected(is_selected),
                                        );
                                        if is_selected {
                                            resp.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        if resp.clicked() {
                                            clicked_icon = Some(cat.icon_index);
                                        }
                                    });
                                }
                            });
                    });
                });

            if let Some(icon) = clicked_icon {
                self.active_dungeon_filters.insert(icon);
                self.dungeon_search_text.clear();
                self.dungeon_search_prev_text.clear();
                self.dungeon_search_selected_index = None;
            }

            if ui.input(|i| i.pointer.any_click()) && !response.has_focus() {
                self.dungeon_search_popup_open = false;
                self.dungeon_search_selected_index = None;
            }
        }
    }

    /// Render the selected forge-category pills. Each pill is a single
    /// fixed-width widget (see [`Self::render_dungeon_pill`]), so the enclosing
    /// `horizontal_wrapped` toolbar flow can measure it and wrap correctly --
    /// pills fill the toolbar line's leftover space first, then wrap. No-op when
    /// nothing is selected.
    fn render_dungeon_pills(
        &mut self,
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        shadcn: &Shadcn,
    ) {
        if self.active_dungeon_filters.is_empty() {
            return;
        }
        let categories = get_asset_manager().dungeon_categories();

        // Stable order by icon index.
        let mut active: Vec<i32> = self.active_dungeon_filters.iter().copied().collect();
        active.sort();

        ui.separator();
        ui.spacing_mut().item_spacing.x = 4.0;

        let mut removed: Option<i32> = None;
        for icon in &active {
            let name = categories
                .iter()
                .find(|c| c.icon_index == *icon)
                .map(|c| c.display_name.clone())
                .unwrap_or_else(|| format!("Category {icon}"));
            if self.render_dungeon_pill(ui, sprite_renderer, *icon, &name) {
                removed = Some(*icon);
            }
        }

        if shadcn
            .btn(ui, "\u{2715}")
            .hover_tip("Clear all category filters")
            .clicked()
        {
            self.active_dungeon_filters.clear();
        } else if let Some(icon) = removed {
            self.active_dungeon_filters.remove(&icon);
        }
    }

    /// Render a removable forge-category pill as one fixed-width widget (icon +
    /// name + ✕). Clicking anywhere on the pill removes that filter. Returning a
    /// single atomic widget (rather than a nested `Frame`/`horizontal`) lets a
    /// wrapping parent layout know the pill's width up front and wrap it cleanly
    /// instead of letting it overflow the right edge. Returns true when clicked.
    fn render_dungeon_pill(
        &self,
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        icon_index: i32,
        name: &str,
    ) -> bool {
        const H: f32 = 20.0;
        const LEFT_PAD: f32 = 6.0;
        const ICON: f32 = 16.0;
        const GAP1: f32 = 4.0; // icon -> text
        const GAP2: f32 = 6.0; // text -> ✕
        const X_W: f32 = 10.0;
        const RIGHT_PAD: f32 = 6.0;

        let font = egui::TextStyle::Small.resolve(ui.style());
        let text_color = ui.visuals().text_color();
        let galley = ui
            .painter()
            .layout_no_wrap(name.to_string(), font.clone(), text_color);
        let text_w = galley.size().x;
        let width = LEFT_PAD + ICON + GAP1 + text_w + GAP2 + X_W + RIGHT_PAD;

        let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, H), egui::Sense::click());
        let hovered = resp.hovered();
        let clicked = resp.clicked();
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        let bg = if hovered {
            Color32::from_rgb(70, 60, 95)
        } else {
            Color32::from_rgb(50, 45, 70)
        };
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(4), bg);

        let icon_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left() + LEFT_PAD, rect.center().y - ICON / 2.0),
            egui::vec2(ICON, ICON),
        );
        sprite_renderer.draw_collection_icon(ui, icon_index, icon_rect);

        let text_pos = egui::pos2(
            icon_rect.right() + GAP1,
            rect.center().y - galley.size().y / 2.0,
        );
        ui.painter().galley(text_pos, galley, text_color);

        let x_center = egui::pos2(rect.right() - RIGHT_PAD - X_W / 2.0, rect.center().y);
        let x_color = if hovered {
            Color32::from_rgb(255, 120, 120)
        } else {
            Color32::from_gray(170)
        };
        ui.painter().text(
            x_center,
            egui::Align2::CENTER_CENTER,
            "\u{2715}",
            font,
            x_color,
        );

        resp.on_hover_text(format!("Remove {name} filter"));
        clicked
    }

    /// Render the category filter bar: a row of category buttons with optional sub-category row.
    /// In Custom sort mode, category buttons are draggable to reorder.
    fn render_category_bar(
        &mut self,
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        shadcn: &Shadcn,
    ) {
        let is_custom = self.sort_mode == TreasurySortMode::Custom;

        // Determine the category order to display
        let categories: Vec<ItemCategory> = if is_custom {
            self.custom_category_order.clone()
        } else {
            ItemCategory::all().to_vec()
        };

        // Collect button rects for drag-drop hit testing (only in Custom mode)
        let mut cat_rects: Vec<(usize, egui::Rect)> = Vec::new();
        let palette = shadcn.colors();

        // Primary category row
        ui.horizontal_wrapped(|ui| {
            ui.set_min_height(28.0);
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);

            // "All" button (clears category filter) -- not draggable
            let all_active = self.active_category.is_none();
            let all_text = RichText::new("All").small();
            let all_text = if all_active {
                all_text.color(palette.accent_foreground).strong()
            } else {
                all_text.color(palette.foreground)
            };

            let all_btn = if all_active {
                egui::Button::new(all_text)
                    .fill(palette.accent)
                    .stroke(egui::Stroke::new(1.0_f32, palette.accent))
            } else {
                egui::Button::new(all_text).fill(palette.secondary)
            };

            if ui.add(all_btn).clicked() {
                self.active_category = None;
                self.active_subcategory = None;
            }

            // One button per category (draggable in Custom mode)
            for (idx, &cat) in categories.iter().enumerate() {
                let is_active = self.active_category == Some(cat);
                let is_being_dragged = is_custom && self.dragging_cat_index == Some(idx);
                let is_drop_target = is_custom && self.cat_drop_target == Some(idx);

                let label = RichText::new(cat.display_name()).small();
                let label = if is_being_dragged {
                    label.color(Color32::from_rgb(255, 200, 50)).strong()
                } else if is_active {
                    label.color(palette.accent_foreground).strong()
                } else {
                    label.color(palette.foreground)
                };

                let btn_fill = if is_being_dragged {
                    Color32::from_rgb(70, 65, 40)
                } else if is_active {
                    palette.accent
                } else {
                    palette.secondary
                };

                let btn_stroke = if is_drop_target {
                    egui::Stroke::new(2.0_f32, Color32::from_rgb(255, 200, 50))
                } else if is_being_dragged {
                    egui::Stroke::new(1.0_f32, Color32::from_rgb(255, 200, 50))
                } else if is_active {
                    egui::Stroke::new(1.0_f32, palette.accent)
                } else {
                    egui::Stroke::NONE
                };

                let sense = if is_custom {
                    egui::Sense::click_and_drag()
                } else {
                    egui::Sense::click()
                };

                let btn = egui::Button::new(label)
                    .fill(btn_fill)
                    .stroke(btn_stroke)
                    .sense(sense);
                let response = ui.add(btn);

                if is_custom {
                    cat_rects.push((idx, response.rect));

                    // Start drag
                    if response.drag_started() {
                        self.dragging_cat_index = Some(idx);
                    }

                    // Change cursor while dragging
                    if is_being_dragged {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    } else if response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                    }
                }

                // Click to filter (works in all modes, but only when not dragging)
                if response.clicked() && self.dragging_cat_index.is_none() {
                    if self.active_category == Some(cat) {
                        self.active_category = None;
                        self.active_subcategory = None;
                    } else {
                        self.active_category = Some(cat);
                        self.active_subcategory = None;
                    }
                }
            }

            // Separator + item-property type filters (UT / ST / Shiny). OR within
            // the group; applies to the totals grid above and the sections below.
            ui.separator();
            Self::prop_button(
                ui,
                shadcn,
                "UT",
                &mut self.props.ut,
                "Only show Untiered (UT) items",
            );
            Self::prop_button(
                ui,
                shadcn,
                "ST",
                &mut self.props.st,
                "Only show Set Tier (ST) items",
            );
            sprite_renderer.shiny_toggle(
                ui,
                shadcn,
                &mut self.props.shiny,
                "Only show shiny items",
            );

            // Separator + rarity filters (by enchant count). Affects the storage
            // sections below only (the aggregated totals grid ignores rarity).
            ui.separator();
            Self::rarity_button(
                ui,
                sprite_renderer,
                None,
                "0",
                &mut self.props.r_none,
                "Sections: show 0-enchant (None) items",
            );
            Self::rarity_button(
                ui,
                sprite_renderer,
                Some(0),
                "1",
                &mut self.props.r_uncommon,
                "Sections: show 1-enchant (Uncommon) items",
            );
            Self::rarity_button(
                ui,
                sprite_renderer,
                Some(1),
                "2",
                &mut self.props.r_rare,
                "Sections: show 2-enchant (Rare) items",
            );
            Self::rarity_button(
                ui,
                sprite_renderer,
                Some(2),
                "3",
                &mut self.props.r_legendary,
                "Sections: show 3-enchant (Legendary) items",
            );
            Self::rarity_button(
                ui,
                sprite_renderer,
                Some(3),
                "4",
                &mut self.props.r_divine,
                "Sections: show 4+-enchant (Divine) items",
            );

            // Separator + Sort mode dropdown
            ui.separator();
            let prev_sort_mode = self.sort_mode;
            let mut sort_key = Some(self.sort_mode.to_key().to_string());
            let sort_options: Vec<(&str, &str)> = TreasurySortMode::all()
                .iter()
                .map(|m| (m.to_key(), m.display_name()))
                .collect();
            shadcn.sel(
                ui,
                "totals_sort_select",
                &mut sort_key,
                120.0,
                &sort_options,
            );
            if let Some(key) = &sort_key {
                if let Some(mode) = TreasurySortMode::from_key(key) {
                    self.sort_mode = mode;
                }
            }
            if self.sort_mode != prev_sort_mode {
                self.sort_settings_dirty = true;
            }

            // Reset custom order button (only in Custom mode)
            if is_custom {
                if shadcn
                    .button_sm(ui, "\u{21BA}")
                    .hover_tip("Reset to default order")
                    .clicked()
                {
                    self.custom_category_order = ItemCategory::all().to_vec();
                    self.sort_settings_dirty = true;
                }
            }
        });

        // Handle drag-drop target detection and release (Custom mode only)
        if is_custom {
            if let Some(dragging_idx) = self.dragging_cat_index {
                if let Some(pointer_pos) = ui.input(|i| i.pointer.interact_pos()) {
                    // Find the closest button the pointer is over
                    let mut target: Option<usize> = None;
                    for &(idx, rect) in &cat_rects {
                        if idx == dragging_idx {
                            continue;
                        }
                        if rect.contains(pointer_pos) {
                            target = Some(idx);
                            break;
                        }
                    }
                    self.cat_drop_target = target;
                }

                // Handle release
                if ui.input(|i| i.pointer.any_released()) {
                    if let Some(target_idx) = self.cat_drop_target {
                        // Move the dragged category to the target position
                        let cat = self.custom_category_order.remove(dragging_idx);
                        self.custom_category_order.insert(target_idx, cat);
                        self.sort_settings_dirty = true;
                    }
                    self.dragging_cat_index = None;
                    self.cat_drop_target = None;
                }
            }
        }

        // Secondary sub-category row (only when a category is selected)
        if let Some(cat) = self.active_category {
            let subcats = ItemSubCategory::for_category(cat);
            if subcats.len() > 1 {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(4.0, 2.0);
                    ui.add_space(8.0); // indent slightly

                    // "All <category>" button (clears sub-category)
                    let all_sub_active = self.active_subcategory.is_none();
                    let all_sub_text = RichText::new(format!("All {}", cat.display_name())).small();
                    let all_sub_text = if all_sub_active {
                        all_sub_text.color(Color32::WHITE).strong()
                    } else {
                        all_sub_text.color(Color32::from_rgb(160, 160, 170))
                    };

                    let all_sub_btn = if all_sub_active {
                        egui::Button::new(all_sub_text)
                            .fill(Color32::from_rgb(50, 50, 60))
                            .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(100, 120, 220)))
                    } else {
                        egui::Button::new(all_sub_text).fill(Color32::from_rgb(35, 35, 40))
                    };

                    if ui.add(all_sub_btn).clicked() {
                        self.active_subcategory = None;
                    }

                    // One button per sub-category
                    for &sub in subcats {
                        let is_active = self.active_subcategory == Some(sub);
                        let label = RichText::new(sub.display_name()).small();
                        let label = if is_active {
                            label.color(Color32::WHITE).strong()
                        } else {
                            label.color(Color32::from_rgb(160, 160, 170))
                        };

                        let btn = if is_active {
                            egui::Button::new(label)
                                .fill(Color32::from_rgb(50, 50, 60))
                                .stroke(egui::Stroke::new(
                                    1.0_f32,
                                    Color32::from_rgb(100, 120, 220),
                                ))
                        } else {
                            egui::Button::new(label).fill(Color32::from_rgb(35, 35, 40))
                        };

                        if ui.add(btn).clicked() {
                            if self.active_subcategory == Some(sub) {
                                // Clicking active sub-category clears it
                                self.active_subcategory = None;
                            } else {
                                self.active_subcategory = Some(sub);
                            }
                        }
                    }
                });
            }
        }
    }

    /// Render a single item-property toggle button (UT / ST / Shiny), gold styling.
    fn prop_button(
        ui: &mut egui::Ui,
        shadcn: &Shadcn,
        label: &str,
        enabled: &mut bool,
        tooltip: &str,
    ) {
        shadcn
            .tgl(ui, enabled, RichText::new(label).small())
            .hover_tip(tooltip);
    }

    /// Render a single rarity toggle button using the enchant-tier sprite.
    ///
    /// `tier_idx` is the enchant overlay index (0=Uncommon..3=Divine), or `None`
    /// for the 0-enchant ("None") rarity which has no sprite. `fallback_label` is
    /// drawn when the sprite is unavailable (overlays not yet loaded, or "None").
    fn rarity_button(
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        tier_idx: Option<usize>,
        fallback_label: &str,
        enabled: &mut bool,
        tooltip: &str,
    ) {
        let size = egui::vec2(18.0, 18.0);
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

        if ui.is_rect_visible(rect) {
            let drawn = match tier_idx {
                Some(idx) => sprite_renderer.draw_enchant_tier_in_rect(ui, idx, rect),
                None => false,
            };

            if !drawn {
                // Fallback: show enchant-count label on a small chip.
                ui.painter()
                    .rect_filled(rect, 2.0, Color32::from_rgb(55, 55, 60));
                let text_color = if *enabled {
                    Color32::WHITE
                } else {
                    Color32::from_rgb(120, 120, 120)
                };
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    fallback_label,
                    egui::FontId::proportional(12.0),
                    text_color,
                );
            }

            // Dimming overlay when disabled (expand to fully cover sprite).
            if !*enabled {
                let expanded_rect = rect.expand(2.0);
                ui.painter().rect_filled(
                    expanded_rect,
                    2.0,
                    Color32::from_rgba_unmultiplied(0, 0, 0, 180),
                );
            }

            // Border when enabled.
            if *enabled {
                ui.painter().rect_stroke(
                    rect,
                    2.0,
                    egui::Stroke::new(1.0_f32, Color32::WHITE),
                    egui::StrokeKind::Outside,
                );
            }
        }

        if response.hover_tip(tooltip).clicked() {
            *enabled = !*enabled;
        }
    }

    /// Render a single collapsible section.
    fn render_section(
        &mut self,
        ui: &mut egui::Ui,
        section: TreasurySection,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        realmhound_core::prof_function!();
        // Use cached section count (rebuilt only when data/filters change)
        let item_count = self
            .cached_section_counts
            .get(&section)
            .copied()
            .unwrap_or(0);

        // Hide sections with zero items (FR-5.4 / task 5.7)
        if item_count == 0 {
            return;
        }

        let header_text = format!("{} ({} items)", section.display_name(), item_count);

        // Seasonal/Regular sections get their accent colors
        let text_color = if section.is_seasonal() {
            SEASONAL_COLOR
        } else {
            REGULAR_COLOR
        };

        // Use CollapsingState for custom header with icon
        let id = egui::Id::new(("treasury_section", section.to_key()));
        let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, true);

        let header_response = ui.horizontal(|ui| {
            // Reserve the full icon height (+ a little breathing room) up front so egui's
            // row bounding rect actually matches what we paint below. The icon is painted
            // directly rather than allocated, so without this the row's tracked height stays
            // at `interact_size.y` while the 24px icon visually overflows it, leaving no gap
            // between consecutive section headers.
            let row_height = TAB_ICON_SIZE + 4.0;
            ui.set_min_height(row_height);

            // Disclosure triangle
            let openness = state.openness(ui.ctx());
            let triangle_size = 8.0;
            let (tri_rect, tri_response) = ui.allocate_exact_size(
                egui::vec2(triangle_size + 4.0, row_height),
                egui::Sense::click(),
            );

            // Draw triangle pointing right (closed) or down (open)
            let center = tri_rect.center();
            let triangle_color = ui.style().visuals.text_color();
            let half = triangle_size / 2.0;

            // Interpolate rotation based on openness
            if openness > 0.5 {
                // Pointing down (open)
                let points = [
                    egui::pos2(center.x - half, center.y - half * 0.5),
                    egui::pos2(center.x + half, center.y - half * 0.5),
                    egui::pos2(center.x, center.y + half * 0.5),
                ];
                ui.painter().add(egui::Shape::convex_polygon(
                    points.to_vec(),
                    triangle_color,
                    egui::Stroke::NONE,
                ));
            } else {
                // Pointing right (closed)
                let points = [
                    egui::pos2(center.x - half * 0.5, center.y - half),
                    egui::pos2(center.x + half * 0.5, center.y),
                    egui::pos2(center.x - half * 0.5, center.y + half),
                ];
                ui.painter().add(egui::Shape::convex_polygon(
                    points.to_vec(),
                    triangle_color,
                    egui::Stroke::NONE,
                ));
            }

            // Section icon
            let icon = get_treasury_section_icon(section);
            let icon_rect = egui::Rect::from_min_size(
                egui::pos2(
                    ui.cursor().min.x,
                    ui.cursor().min.y + (row_height - TAB_ICON_SIZE) / 2.0,
                ),
                egui::vec2(TAB_ICON_SIZE, TAB_ICON_SIZE),
            );
            sprite_renderer.render_icon(ui, Some(icon), icon_rect);
            ui.add_space(TAB_ICON_SIZE + 4.0);

            // Header text
            ui.label(
                RichText::new(header_text)
                    .strong()
                    .size(15.0)
                    .color(text_color),
            );

            tri_response
        });

        // Toggle on click
        if header_response.inner.clicked() {
            state.toggle(ui);
        }

        // Show body if open
        state.show_body_unindented(ui, |ui| {
            self.render_section_dispatch(ui, section, &self.active_item_filters, sprite_renderer);
        });
    }

    /// Dispatch section rendering based on section type and optional item filters.
    ///
    /// Unified handler for both normal and filtered modes:
    /// - Characters/Pets: always pass filters through (render methods handle both cases)
    /// - Large containers (Vault, Gift, Spoils): each matching slot rendered individually when filtered
    /// - Small containers (Materials, Potions): full grid with highlight when filtered (FR-5.3)
    fn render_section_dispatch(
        &self,
        ui: &mut egui::Ui,
        section: TreasurySection,
        filter_items: &HashSet<i32>,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        realmhound_core::prof_function!();
        match section {
            TreasurySection::SeasonalCharacters => {
                self.render_character_section(ui, true, filter_items, sprite_renderer);
            }
            TreasurySection::RegularCharacters => {
                self.render_character_section(ui, false, filter_items, sprite_renderer);
            }
            TreasurySection::SeasonalPetInventories => {
                self.render_pet_section(ui, true, filter_items, sprite_renderer);
            }
            TreasurySection::RegularPetInventories => {
                self.render_pet_section(ui, false, filter_items, sprite_renderer);
            }
            // Large containers: each matching slot rendered individually (compact) when any
            // filter is active, full grid otherwise
            TreasurySection::SeasonalVault
            | TreasurySection::RegularVault
            | TreasurySection::SeasonalGiftChests
            | TreasurySection::RegularGiftChest
            | TreasurySection::SeasonalSpoils => {
                if self.section_filter_active(filter_items) {
                    self.render_vault_filtered_slots(ui, section, filter_items, sprite_renderer);
                } else {
                    let (seasonal, accessor, grid_id): (
                        bool,
                        fn(&LiveVaultStorage) -> &Vec<LiveVaultItem>,
                        &str,
                    ) = match section {
                        TreasurySection::SeasonalVault => {
                            (true, |s| &s.vault_items, "seasonal_vault")
                        }
                        TreasurySection::RegularVault => {
                            (false, |s| &s.vault_items, "regular_vault")
                        }
                        TreasurySection::SeasonalGiftChests => {
                            (true, |s| &s.gift_items, "seasonal_gifts")
                        }
                        TreasurySection::RegularGiftChest => {
                            (false, |s| &s.gift_items, "regular_gifts")
                        }
                        // Seasonal spoils are stored in REGULAR vault (rewards from seasonal mode)
                        TreasurySection::SeasonalSpoils => {
                            (false, |s| &s.spoils_items, "seasonal_spoils")
                        }
                        _ => unreachable!(),
                    };
                    self.render_vault_section(
                        ui,
                        seasonal,
                        accessor,
                        grid_id,
                        filter_items,
                        sprite_renderer,
                    );
                }
            }
            // Small containers: full grid (with highlight when filtered)
            TreasurySection::SeasonalMaterials => {
                self.render_vault_section(
                    ui,
                    true,
                    |s| &s.material_items,
                    "seasonal_materials",
                    filter_items,
                    sprite_renderer,
                );
            }
            TreasurySection::RegularMaterials => {
                self.render_vault_section(
                    ui,
                    false,
                    |s| &s.material_items,
                    "regular_materials",
                    filter_items,
                    sprite_renderer,
                );
            }
            TreasurySection::SeasonalPotionRack => {
                self.render_vault_section(
                    ui,
                    true,
                    |s| &s.potion_items,
                    "seasonal_potions",
                    filter_items,
                    sprite_renderer,
                );
            }
            TreasurySection::RegularPotionRack => {
                self.render_vault_section(
                    ui,
                    false,
                    |s| &s.potion_items,
                    "regular_potions",
                    filter_items,
                    sprite_renderer,
                );
            }
        }
    }

    /// Count how many items in a section match the given filter item_id,
    /// also applying the active category filter.
    ///
    /// Uses `items_match()` so that filtering by a stackable base item
    /// (e.g., "The Fool Tarot Card x1") matches all stack variants.
    fn section_filter_match_count(&self, section: TreasurySection, filter_id: i32) -> usize {
        // If the filtered item doesn't match the active category or shiny filter, count is always 0
        if !self.item_matches_filters(filter_id) {
            return 0;
        }

        let am = get_asset_manager();

        match section {
            TreasurySection::SeasonalCharacters | TreasurySection::RegularCharacters => {
                let seasonal = matches!(section, TreasurySection::SeasonalCharacters);
                self.character_cache
                    .as_ref()
                    .map(|cache| {
                        cache
                            .characters
                            .iter()
                            .filter(|c| {
                                c.seasonal == seasonal && Self::character_has_item(c, filter_id, am)
                            })
                            .map(|c| self.character_item_count_for(c, filter_id, am))
                            .sum()
                    })
                    .unwrap_or(0)
            }
            TreasurySection::SeasonalPetInventories | TreasurySection::RegularPetInventories => {
                let seasonal = matches!(section, TreasurySection::SeasonalPetInventories);
                self.character_cache
                    .as_ref()
                    .map(|cache| {
                        let pets = if seasonal {
                            &cache.seasonal_pets
                        } else {
                            &cache.regular_pets
                        };
                        pets.values()
                            .filter(|p| p.has_inventory())
                            .flat_map(|p| p.inventory.iter())
                            // Pet items carry no enchant data -> rarity bucket None.
                            .filter(|i| {
                                am.items_match(i.item_id, filter_id) && self.props.passes_rarity(0)
                            })
                            .map(|i| {
                                let (_base, stack) = am.get_stack_info(i.item_id);
                                stack as usize
                            })
                            .sum()
                    })
                    .unwrap_or(0)
            }
            _ => self
                .vault_section_items(section)
                .map(|items| {
                    items
                        .iter()
                        .filter(|i| {
                            am.items_match(i.item_id, filter_id)
                                && self.props.passes_rarity(i.enchant_ids.len())
                        })
                        .map(|i| {
                            let (_base, stack) = am.get_stack_info(i.item_id);
                            stack as usize
                        })
                        .sum()
                })
                .unwrap_or(0),
        }
    }

    // -----------------------------------------------------------------------
    // Section item counts
    // -----------------------------------------------------------------------

    /// Count non-empty items for a given section, applying the active category filter.
    fn section_item_count(&self, section: TreasurySection) -> usize {
        match section {
            TreasurySection::SeasonalCharacters | TreasurySection::RegularCharacters => {
                let seasonal = matches!(section, TreasurySection::SeasonalCharacters);
                self.character_cache
                    .as_ref()
                    .map(|cache| {
                        cache
                            .characters
                            .iter()
                            .filter(|c| c.seasonal == seasonal)
                            .map(|c| self.character_item_count_with_category(c))
                            .sum()
                    })
                    .unwrap_or(0)
            }
            TreasurySection::SeasonalPetInventories | TreasurySection::RegularPetInventories => {
                let seasonal = matches!(section, TreasurySection::SeasonalPetInventories);
                self.character_cache
                    .as_ref()
                    .map(|cache| {
                        let pets = if seasonal {
                            &cache.seasonal_pets
                        } else {
                            &cache.regular_pets
                        };
                        pets.values()
                            .filter(|p| p.has_inventory())
                            .map(|p| {
                                p.inventory
                                    .iter()
                                    .filter(|i| {
                                        // Pet items carry no enchant data -> rarity bucket None.
                                        i.item_id >= 0 && self.item_instance_matches(i.item_id, 0)
                                    })
                                    .count()
                            })
                            .sum()
                    })
                    .unwrap_or(0)
            }
            _ => {
                // Vault-type sections
                self.vault_section_items(section)
                    .map(|items| {
                        items
                            .iter()
                            .filter(|i| {
                                !i.is_empty()
                                    && self.item_instance_matches(i.item_id, i.enchant_ids.len())
                            })
                            .count()
                    })
                    .unwrap_or(0)
            }
        }
    }

    /// Get a reference to the vault items for a given vault-type section.
    fn vault_section_items(&self, section: TreasurySection) -> Option<&[LiveVaultItem]> {
        let data = self.live_vault_data.as_ref()?;
        let (storage, accessor): (
            &LiveVaultStorage,
            fn(&LiveVaultStorage) -> &Vec<LiveVaultItem>,
        ) = match section {
            TreasurySection::SeasonalVault => (&data.seasonal, |s| &s.vault_items),
            TreasurySection::SeasonalMaterials => (&data.seasonal, |s| &s.material_items),
            TreasurySection::SeasonalPotionRack => (&data.seasonal, |s| &s.potion_items),
            TreasurySection::SeasonalGiftChests => (&data.seasonal, |s| &s.gift_items),
            // Seasonal spoils are stored in REGULAR vault (they're rewards from seasonal mode)
            TreasurySection::SeasonalSpoils => (&data.regular, |s| &s.spoils_items),
            TreasurySection::RegularVault => (&data.regular, |s| &s.vault_items),
            TreasurySection::RegularMaterials => (&data.regular, |s| &s.material_items),
            TreasurySection::RegularPotionRack => (&data.regular, |s| &s.potion_items),
            TreasurySection::RegularGiftChest => (&data.regular, |s| &s.gift_items),
            _ => return None,
        };
        Some(accessor(storage))
    }

    /// Count non-empty items on a character that match the active category filter.
    ///
    /// When no category is active, counts all non-empty items.
    fn character_item_count_with_category(&self, c: &CachedCharacter) -> usize {
        let count_matching = |items: &[CharacterItem]| -> usize {
            items
                .iter()
                .filter(|i| {
                    !i.is_empty() && self.item_instance_matches(i.item_id, i.enchant_ids.len())
                })
                .count()
        };
        count_matching(&c.equipment)
            + count_matching(&c.inventory)
            + count_matching(&c.backpack)
            + count_matching(&c.backpack_ext)
            + count_matching(&c.belt)
    }

    /// Check whether a character has a specific item in any slot.
    ///
    /// Uses `items_match()` to support stackable variant matching.
    fn character_has_item(
        c: &CachedCharacter,
        item_id: i32,
        am: &realmhound_core::assets::AssetManager,
    ) -> bool {
        let contains =
            |items: &[CharacterItem]| items.iter().any(|i| am.items_match(i.item_id, item_id));
        contains(&c.equipment)
            || contains(&c.inventory)
            || contains(&c.backpack)
            || contains(&c.backpack_ext)
            || contains(&c.belt)
    }

    /// Count how many of a specific item a character has across all slots,
    /// applying the per-instance rarity filter.
    ///
    /// Uses `items_match()` to support stackable variant matching.
    fn character_item_count_for(
        &self,
        c: &CachedCharacter,
        item_id: i32,
        am: &realmhound_core::assets::AssetManager,
    ) -> usize {
        let count = |items: &[CharacterItem]| {
            items
                .iter()
                .filter(|i| {
                    am.items_match(i.item_id, item_id)
                        && self.props.passes_rarity(i.enchant_ids.len())
                })
                .count()
        };
        count(&c.equipment)
            + count(&c.inventory)
            + count(&c.backpack)
            + count(&c.backpack_ext)
            + count(&c.belt)
    }

    /// Whether any slot in `items` is shown under the active filters: non-empty,
    /// matching one of `filter_items` (when that set is non-empty, stackable-aware),
    /// and passing the type/category/text/rarity filters for its instance.
    fn slots_have_match(
        &self,
        items: &[CharacterItem],
        filter_items: &HashSet<i32>,
        am: &AssetManager,
    ) -> bool {
        items.iter().any(|i| {
            !i.is_empty()
                && (filter_items.is_empty()
                    || filter_items
                        .iter()
                        .any(|&fid| am.items_match(i.item_id, fid)))
                && self.item_instance_matches(i.item_id, i.enchant_ids.len())
        })
    }

    /// Whether a character has at least one item visible under the active filters.
    fn character_has_visible_item(
        &self,
        c: &CachedCharacter,
        filter_items: &HashSet<i32>,
        am: &AssetManager,
    ) -> bool {
        self.slots_have_match(&c.equipment, filter_items, am)
            || self.slots_have_match(&c.inventory, filter_items, am)
            || self.slots_have_match(&c.backpack, filter_items, am)
            || self.slots_have_match(&c.backpack_ext, filter_items, am)
            || self.slots_have_match(&c.belt, filter_items, am)
    }

    /// Whether a pet has at least one inventory item visible under the active
    /// filters. Pet items carry no enchant data, so rarity is evaluated as None.
    fn pet_has_visible_item(
        &self,
        p: &CachedPet,
        filter_items: &HashSet<i32>,
        am: &AssetManager,
    ) -> bool {
        p.inventory.iter().any(|i| {
            i.item_id >= 0
                && (filter_items.is_empty()
                    || filter_items
                        .iter()
                        .any(|&fid| am.items_match(i.item_id, fid)))
                && self.item_instance_matches(i.item_id, 0)
        })
    }

    /// Whether any item-property filter (item-id type group, rarity, category, or
    /// text) is currently active and should narrow the bottom sections.
    fn section_filter_active(&self, filter_items: &HashSet<i32>) -> bool {
        !filter_items.is_empty()
            || self.active_category.is_some()
            || self.active_text_filter.is_some()
            || !self.active_dungeon_filters.is_empty()
            || self.props.type_active()
            || self.props.rarity_constraining()
    }

    // -----------------------------------------------------------------------
    // Character section rendering
    // -----------------------------------------------------------------------

    /// Render a character section with compact character cards in a wrapping grid.
    ///
    /// When `filter_items` is non-empty, only characters containing any of those items are shown (FR-5.1).
    /// When a category filter is active, only characters with matching items are shown.
    fn render_character_section(
        &self,
        ui: &mut egui::Ui,
        seasonal: bool,
        filter_items: &HashSet<i32>,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        let cache = match &self.character_cache {
            Some(c) => c,
            None => {
                Self::render_no_data(ui, "Fetch characters to populate.");
                return;
            }
        };

        let all_chars: Vec<&CachedCharacter> = cache.get_sorted_characters(seasonal);
        let am = get_asset_manager();
        // When any filter is active, only show characters that have at least one
        // item visible under those filters (item selection, type, rarity, category).
        let chars: Vec<&CachedCharacter> = if self.section_filter_active(filter_items) {
            all_chars
                .into_iter()
                .filter(|c| self.character_has_visible_item(c, filter_items, am))
                .collect()
        } else {
            all_chars
        };

        if chars.is_empty() {
            Self::render_no_data(ui, "No characters found.");
            return;
        }

        let card_width = CARD_CONTENT_WIDTH + CARD_PADDING * 2.0 + 4.0; // +4 for stroke (2px each side)

        // Horizontal wrapping grid with fixed-width cards (same as Characters tab)
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(CARD_SPACING, CARD_SPACING);
            for char in &chars {
                let custom_label = cache.custom_labels.get(&char.char_id).map(|s| s.as_str());

                // Card frame (same style as Characters panel but no drag/context menu)
                let fill = crate::ui_colors::card_fill(ui.visuals());
                let stroke_color = crate::ui_colors::card_stroke(ui.visuals());

                // allocate_ui reserves fixed width so horizontal_wrapped can measure
                // and break to the next row correctly (same pattern as Characters tab)
                ui.allocate_ui(egui::vec2(card_width, ui.available_height()), |ui| {
                    egui::Frame::NONE
                        .fill(fill)
                        .stroke(egui::Stroke::new(1.0_f32, stroke_color))
                        .corner_radius(8.0)
                        .inner_margin(CARD_PADDING)
                        .show(ui, |ui| {
                            ui.set_min_width(CARD_CONTENT_WIDTH);
                            ui.set_max_width(CARD_CONTENT_WIDTH);

                            ui.vertical(|ui| {
                                // Pass all filters for highlight (OR logic)
                                let clicked = CharacterCardWidget::compact()
                                    .with_highlights(filter_items)
                                    .clickable()
                                    .render(
                                        ui,
                                        char,
                                        false, // not live (totals is a static view)
                                        custom_label,
                                        sprite_renderer,
                                    );
                                if let Some(id) = clicked {
                                    self.pending_item_click.set(Some(id));
                                }
                            });
                        });
                });
            }
        });
    }

    // -----------------------------------------------------------------------
    // Pet section rendering
    // -----------------------------------------------------------------------

    /// Render a pet section with compact pet cards in a wrapping grid.
    ///
    /// When `filter_items` is non-empty, shows full contents with highlighted items (FR-5.3).
    fn render_pet_section(
        &self,
        ui: &mut egui::Ui,
        seasonal: bool,
        filter_items: &HashSet<i32>,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        let cache = match &self.character_cache {
            Some(c) => c,
            None => {
                Self::render_no_data(ui, "Fetch characters to populate.");
                return;
            }
        };

        let pets = if seasonal {
            &cache.seasonal_pets
        } else {
            &cache.regular_pets
        };

        if pets.is_empty() {
            Self::render_no_data(ui, "No pets found.");
            return;
        }

        // Sort pets by instance_id for stable ordering, filtering out pets without inventory unlocked
        let am = get_asset_manager();
        let mut sorted_pets: Vec<&CachedPet> =
            pets.values().filter(|p| p.has_inventory()).collect();
        sorted_pets.sort_by_key(|p| p.instance_id);

        // When any filter is active, only show pets with at least one item visible
        // under those filters (item selection, type, rarity, category).
        if self.section_filter_active(filter_items) {
            sorted_pets.retain(|p| self.pet_has_visible_item(p, filter_items, am));
        }

        if sorted_pets.is_empty() {
            Self::render_no_data(ui, "No matching pets.");
            return;
        }

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(CARD_SPACING, CARD_SPACING);
            for pet in &sorted_pets {
                // Pass all filters for highlight (OR logic)
                PetCardWidget::render(
                    ui,
                    pet,
                    filter_items,
                    true,
                    &self.pending_item_click,
                    sprite_renderer,
                );
            }
        });
    }

    // -----------------------------------------------------------------------
    // Vault/container section rendering
    // -----------------------------------------------------------------------

    /// Render a vault-type section (vault, gift, materials, potions, spoils) in chest-of-8 layout.
    ///
    /// When `highlight_items` is non-empty, matching slots get a colored border highlight (FR-5.3).
    /// When a category filter is active, non-matching items are rendered as empty slots.
    fn render_vault_section(
        &self,
        ui: &mut egui::Ui,
        seasonal: bool,
        accessor: fn(&LiveVaultStorage) -> &Vec<LiveVaultItem>,
        grid_id: &str,
        highlight_items: &HashSet<i32>,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        let data = match &self.live_vault_data {
            Some(d) => d,
            None => {
                Self::render_no_data(ui, "Capture vault data to populate.");
                return;
            }
        };

        let storage = if seasonal {
            &data.seasonal
        } else {
            &data.regular
        };
        let items = accessor(storage);

        if items.is_empty() {
            Self::render_no_data(ui, "No items in this storage.");
            return;
        }

        // When category/shiny/text filter is active, pass filter params to grid renderer
        let category_filter: Option<&ItemCategorizer> = if self.active_category.is_some() {
            self.categorizer.as_ref()
        } else {
            None
        };
        let active_cat = self.active_category;
        let active_sub = self.active_subcategory;
        let props = self.props;
        let text_filter = self.active_text_filter.as_deref();

        Self::render_item_grid_inline(
            ui,
            grid_id,
            items,
            highlight_items,
            category_filter,
            active_cat,
            active_sub,
            props,
            text_filter,
            &self.pending_item_click,
            sprite_renderer,
        );
    }

    /// Render each matching slot in a large container individually when filtered.
    ///
    /// Previously these large containers (vault, gift chests, spoils) collapsed
    /// matches into a condensed "[icon] xN" summary. Instead, show every matching
    /// vault slot as its own tile so per-item stats (enchants) are visible. Each
    /// physical vault slot is rendered separately, so stackable items (e.g.
    /// schematics) stay as one tile per slot they occupy, with the stack count
    /// drawn on the tile.
    fn render_vault_filtered_slots(
        &self,
        ui: &mut egui::Ui,
        section: TreasurySection,
        filter_items: &HashSet<i32>,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        let am = get_asset_manager();
        let items = match self.vault_section_items(section) {
            Some(items) => items,
            None => {
                Self::render_no_data(ui, "Capture vault data to populate.");
                return;
            }
        };

        const CELL_SIZE: f32 = 38.0;

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
            let mut any = false;
            for item in items {
                if item.is_empty() {
                    continue;
                }
                if !filter_items.is_empty()
                    && !filter_items
                        .iter()
                        .any(|&id| am.items_match(item.item_id, id))
                {
                    continue;
                }
                if !self.item_instance_matches(item.item_id, item.enchant_ids.len()) {
                    continue;
                }
                // allocate_ui reserves a fixed size so horizontal_wrapped can
                // measure and break to the next row correctly
                ui.allocate_ui(egui::vec2(CELL_SIZE, CELL_SIZE), |ui| {
                    Self::render_vault_item_slot(
                        ui,
                        item,
                        !filter_items.is_empty(),
                        false,
                        true,
                        &self.pending_item_click,
                        sprite_renderer,
                    );
                });
                any = true;
            }
            if !any {
                ui.label(
                    RichText::new("No matching items")
                        .color(Color32::GRAY)
                        .italics(),
                );
            }
        });
    }

    /// Render a chest-of-8 item grid inline (no scroll area) with multi-column
    /// layout matching the Vault panel approach.
    ///
    /// When `highlight_items` is non-empty, matching slots get a colored border.
    /// When `category_filter` is Some, non-matching items render as empty slots
    /// (avoids cloning the entire item vec).
    fn render_item_grid_inline(
        ui: &mut egui::Ui,
        grid_id: &str,
        items: &[LiveVaultItem],
        highlight_items: &HashSet<i32>,
        category_filter: Option<&ItemCategorizer>,
        active_cat: Option<ItemCategory>,
        active_sub: Option<ItemSubCategory>,
        props: PropFilter,
        text_filter: Option<&str>,
        click_sink: &std::cell::Cell<Option<i32>>,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        // Helper: should this item be shown or masked as empty?
        let am = get_asset_manager();
        let text_lower = text_filter.map(|t| t.to_lowercase());
        let item_visible = |item: &LiveVaultItem| -> bool {
            if item.is_empty() {
                return true; // empty stays empty
            }
            // Type filter (UT/ST/Shiny) + per-instance rarity filter.
            if !props.passes(am, item.item_id, item.enchant_ids.len()) {
                return false;
            }
            // Text filter
            if let Some(ref text) = text_lower {
                let name = am
                    .object_name(item.item_id)
                    .unwrap_or_default()
                    .to_lowercase();
                if !name.contains(text.as_str()) {
                    return false;
                }
            }
            if let (Some(cz), Some(cat)) = (category_filter, active_cat) {
                let (item_cat, item_sub) = cz.get(item.item_id);
                if item_cat != cat {
                    return false;
                }
                if let Some(sub) = active_sub {
                    if item_sub != sub {
                        return false;
                    }
                }
            }
            true
        };

        let cell_size = 38.0 + 8.0; // item_size + grid spacing
        let single_column_width = 8.0 * cell_size;
        let column_gap = 16.0;
        let available_width = ui.available_width();

        // Calculate how many 8-wide columns fit side by side
        let max_cols = ((available_width + column_gap) / (single_column_width + column_gap))
            .floor()
            .max(1.0) as usize;

        let total_rows = (items.len() + 7) / 8;

        if max_cols <= 1 || total_rows <= 1 {
            // Single column: simple grid
            egui::Grid::new(grid_id)
                .num_columns(8)
                .spacing([8.0, 8.0])
                .show(ui, |ui| {
                    for (idx, item) in items.iter().enumerate() {
                        if item_visible(item) {
                            let highlight = !highlight_items.is_empty()
                                && highlight_items
                                    .iter()
                                    .any(|&id| get_asset_manager().items_match(item.item_id, id));
                            Self::render_vault_item_slot(
                                ui,
                                item,
                                highlight,
                                !highlight_items.is_empty(),
                                true,
                                click_sink,
                                sprite_renderer,
                            );
                        } else {
                            sprite_renderer.render_empty_slot(ui, 38.0);
                        }
                        if (idx + 1) % 8 == 0 {
                            ui.end_row();
                        }
                    }
                });
        } else {
            // Multi-column: distribute rows evenly across columns
            let rows_per_col = (total_rows + max_cols - 1) / max_cols;
            let items_per_col = rows_per_col * 8;

            ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                let mut offset = 0;
                let mut col_idx = 0;
                while offset < items.len() {
                    let end = (offset + items_per_col).min(items.len());
                    let col_items = &items[offset..end];

                    egui::Grid::new(format!("{}_{}", grid_id, col_idx))
                        .num_columns(8)
                        .spacing([8.0, 8.0])
                        .show(ui, |ui| {
                            for (idx, item) in col_items.iter().enumerate() {
                                if item_visible(item) {
                                    let highlight = !highlight_items.is_empty()
                                        && highlight_items.iter().any(|&id| {
                                            get_asset_manager().items_match(item.item_id, id)
                                        });
                                    Self::render_vault_item_slot(
                                        ui,
                                        item,
                                        highlight,
                                        !highlight_items.is_empty(),
                                        true,
                                        click_sink,
                                        sprite_renderer,
                                    );
                                } else {
                                    sprite_renderer.render_empty_slot(ui, 38.0);
                                }
                                if (idx + 1) % 8 == 0 {
                                    ui.end_row();
                                }
                            }
                        });

                    if end < items.len() {
                        ui.add_space(column_gap);
                    }

                    offset = end;
                    col_idx += 1;
                }
            });
        }
    }

    /// Render a single vault item slot.
    ///
    /// When `highlight` is true, draws a colored border around the slot.
    fn render_vault_item_slot(
        ui: &mut egui::Ui,
        item: &LiveVaultItem,
        highlight: bool,
        search_active: bool,
        clickable: bool,
        click_sink: &std::cell::Cell<Option<i32>>,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        if item.is_empty() {
            sprite_renderer.render_empty_slot(ui, 38.0);
        } else {
            let mut enchant_buf: [i32; 4] = [0; 4];
            let enchant_count = item.enchant_ids.len().min(4);
            for (i, &e) in item.enchant_ids.iter().take(4).enumerate() {
                enchant_buf[i] = e as i32;
            }
            let response = sprite_renderer.render_item_tile_ex(
                ui,
                item.item_id,
                None,
                &enchant_buf[..enchant_count],
                highlight,
                clickable,
            );
            if clickable && response.clicked() {
                click_sink.set(Some(item.item_id));
            }
            if search_active && !highlight {
                ui.painter()
                    .rect_filled(response.rect, 4.0, crate::ui_colors::DIM_OVERLAY);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Whether an item passes the active forge/tooltip category filter. Returns
    /// true when no category filter is active. Otherwise the item's in-game
    /// `collectionIcon` index must be one of the selected categories (OR logic).
    fn passes_dungeon_category(&self, item_id: i32) -> bool {
        if self.active_dungeon_filters.is_empty() {
            return true;
        }
        match get_asset_manager().item_category_icon(item_id) {
            Some(icon) => self.active_dungeon_filters.contains(&icon),
            None => false,
        }
    }

    /// Check if an item matches the active category (and sub-category) filter,
    /// the type group (UT/ST/Shiny), and the text filter. This is item-id level
    /// only and does NOT consider rarity (which is per-instance).
    ///
    /// Returns true when no filters are active, or when the item passes all
    /// active filters.
    fn item_matches_filters(&self, item_id: i32) -> bool {
        // Forge/tooltip category filter (OR within group, in-game data)
        if !self.passes_dungeon_category(item_id) {
            return false;
        }
        // Type filter (UT/ST/Shiny, OR within group)
        if !self.props.passes_type(get_asset_manager(), item_id) {
            return false;
        }
        // Text filter (orthogonal)
        if let Some(ref text) = self.active_text_filter {
            let name = get_asset_manager()
                .object_name(item_id)
                .unwrap_or_default()
                .to_lowercase();
            if !name.contains(&text.to_lowercase()) {
                return false;
            }
        }
        let cat = match self.active_category {
            Some(c) => c,
            None => return true,
        };
        if let Some(ref cz) = self.categorizer {
            let (item_cat, item_sub) = cz.get(item_id);
            if item_cat != cat {
                return false;
            }
            if let Some(sub) = self.active_subcategory {
                if item_sub != sub {
                    return false;
                }
            }
            true
        } else {
            true
        }
    }

    /// Per-instance filter check: the item-id level filters (type/text/category)
    /// plus the rarity group applied to this instance's `enchant_count`. Used by
    /// the bottom sections, which have per-instance enchant data.
    fn item_instance_matches(&self, item_id: i32, enchant_count: usize) -> bool {
        self.item_matches_filters(item_id) && self.props.passes_rarity(enchant_count)
    }

    /// Render a "no data" placeholder message.
    fn render_no_data(ui: &mut egui::Ui, message: &str) {
        ui.label(RichText::new(message).color(Color32::GRAY).italics());
    }
}

// ---------------------------------------------------------------------------
// Panel trait implementation
// ---------------------------------------------------------------------------

impl Panel for TreasuryPanel {
    fn show(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext) -> Vec<AppAction> {
        self.render(ui, ctx);
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::PropFilter;

    /// Build a rarity-only filter (type filters off) selecting exactly the given tiers.
    fn rarity_only(none: bool, unc: bool, rare: bool, leg: bool, div: bool) -> PropFilter {
        PropFilter {
            ut: false,
            st: false,
            shiny: false,
            r_none: none,
            r_uncommon: unc,
            r_rare: rare,
            r_legendary: leg,
            r_divine: div,
        }
    }

    #[test]
    fn default_imposes_no_constraint() {
        let p = PropFilter::default();
        assert!(!p.type_active());
        // All rarity buttons on => no rarity constraint => every enchant count passes.
        assert!(!p.rarity_constraining());
        for n in 0..6 {
            assert!(p.passes_rarity(n));
        }
    }

    #[test]
    fn rarity_all_on_equals_all_off() {
        let all_on = rarity_only(true, true, true, true, true);
        let all_off = rarity_only(false, false, false, false, false);
        assert!(!all_on.rarity_constraining());
        assert!(!all_off.rarity_constraining());
        for n in 0..6 {
            assert!(all_on.passes_rarity(n));
            assert!(all_off.passes_rarity(n));
        }
    }

    #[test]
    fn rarity_maps_enchant_count_to_tier() {
        // Rare = exactly 2 enchants.
        let p = rarity_only(false, false, true, false, false);
        assert!(p.rarity_constraining());
        assert!(!p.passes_rarity(1));
        assert!(p.passes_rarity(2));
        assert!(!p.passes_rarity(3));

        // Divine = 4 or more enchants.
        let p = rarity_only(false, false, false, false, true);
        assert!(!p.passes_rarity(3));
        assert!(p.passes_rarity(4));
        assert!(p.passes_rarity(7));

        // None = 0 enchants.
        let p = rarity_only(true, false, false, false, false);
        assert!(p.passes_rarity(0));
        assert!(!p.passes_rarity(1));
    }

    #[test]
    fn rarity_or_within_group() {
        // None + Divine selected: 0 and 4+ pass; 1/2/3 do not.
        let p = rarity_only(true, false, false, false, true);
        assert!(p.passes_rarity(0));
        assert!(p.passes_rarity(5));
        assert!(!p.passes_rarity(1));
        assert!(!p.passes_rarity(2));
        assert!(!p.passes_rarity(3));
    }

    #[test]
    fn type_active_reflects_buttons() {
        assert!(!PropFilter::default().type_active());
        let mut p = PropFilter::default();
        p.ut = true;
        assert!(p.type_active());
    }
}

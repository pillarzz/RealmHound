//! Vault/Storage panel - displays account storage from RotMG API and live packet capture.

use eframe::egui::{self, Color32, RichText, ScrollArea};
use realmhound_core::{
    api::{parse_account_data, AccountData, Pet, PetInventoryItem, RotmgApiClient, StorageItem},
    assets::get_asset_manager,
    vault::{CachedPet, CharacterCache, LiveVaultData, LiveVaultItem, LiveVaultStorage, VaultType},
};
use std::sync::mpsc;
use std::thread;
use std::time::SystemTime;

use crate::panels::{AppAction, Panel, PanelContext};
use crate::rendering::SpriteRenderer;
use crate::shadcn_ui::Shadcn;
use crate::tab_icons::{
    get_vault_materials_icon, get_vault_pet_inventories_icon, get_vault_potions_icon,
    get_vault_subtab_icon, get_vault_top_tab_icon, TabIconSprite,
};
use crate::ui_ext::HoverTooltipExt;

/// Top-level vault tabs (regular vs seasonal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultTopTab {
    Regular,
    Seasonal,
}

/// Sub-tabs within each vault type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultSubTab {
    Vault,
    Gifts,
    Storage, // Combined Potions + Materials
    Spoils,  // Seasonal spoils (only in regular vault)
}

/// Data source indicator for vault display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSource {
    /// Live data from packet capture
    Live,
    /// Cached data from disk
    Cached,
    /// Data from RotMG API
    Api,
    /// No data available
    None,
}

/// State of the API fetch operation.
#[derive(Debug, Clone)]
pub enum VaultFetchState {
    /// No token captured yet
    NoToken,
    /// Waiting for API response
    Loading,
    /// Successfully loaded account data (with optional cache timestamp)
    Loaded {
        data: Box<AccountData>,
        cached_at: Option<SystemTime>,
    },
    /// Error occurred
    Error(String),
}

/// Vault panel state.
pub struct VaultPanel {
    /// Current fetch state
    fetch_state: VaultFetchState,
    /// Active top-level tab (regular vs seasonal)
    active_top_tab: VaultTopTab,
    /// Active sub-tab for regular vault
    regular_sub_tab: VaultSubTab,
    /// Active sub-tab for seasonal vault
    seasonal_sub_tab: VaultSubTab,
    /// Receiver for API results (AccountData, optional XML for caching)
    api_result_rx: Option<mpsc::Receiver<Result<(AccountData, Option<String>), String>>>,
    /// Live vault data from packet capture
    live_vault_data: Option<LiveVaultData>,
    /// Whether live vault data was loaded from cache (vs captured live)
    live_vault_from_cache: bool,
    /// Capture time of the current token (if known), refreshed each frame from
    /// `PanelContext`; enriches token-expiry messages.
    token_captured_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Functional `char_list.xml` cache path. Injected so the panel resolves no
    /// path itself; this is the functional API cache, split from debug-only API
    /// response diagnostics.
    char_list_cache_path: std::path::PathBuf,
}

impl VaultPanel {
    /// Create a new vault panel with live vault data from AccountData and an
    /// explicit functional `char_list.xml` cache path.
    ///
    /// The live vault data is provided by AccountData to ensure single source of truth.
    pub fn with_live_vault_data(
        live_vault_data: LiveVaultData,
        char_list_cache_path: std::path::PathBuf,
    ) -> Self {
        // Try to load cached account data (API data for gifts, etc.)
        let fetch_state = match Self::load_cached_data(&char_list_cache_path) {
            Some((data, cached_at)) => VaultFetchState::Loaded {
                data: Box::new(data),
                cached_at: Some(cached_at),
            },
            None => VaultFetchState::NoToken,
        };

        let has_data = live_vault_data.has_any_data();

        Self {
            fetch_state,
            active_top_tab: VaultTopTab::Seasonal,
            regular_sub_tab: VaultSubTab::Vault,
            seasonal_sub_tab: VaultSubTab::Vault,
            api_result_rx: None,
            live_vault_data: Some(live_vault_data),
            live_vault_from_cache: has_data, // Mark as cached if we have data
            token_captured_at: None,
            char_list_cache_path,
        }
    }

    /// Set live vault data captured from packets.
    ///
    /// The canonical `AccountData` (single source of truth) persists the vault;
    /// there is no separate runtime `live_vault.json` writer.
    pub fn set_live_vault_data(&mut self, data: LiveVaultData) {
        self.live_vault_data = Some(data);
        self.live_vault_from_cache = false; // This is fresh live data
    }

    /// Update a pet's inventory slot for real-time updates.
    /// Returns true if the update was successful.
    pub fn update_pet_inventory_slot(
        &mut self,
        pet_instance_id: i32,
        seasonal: bool,
        slot: usize,
        item_id: i32,
        stack_count: u8,
    ) -> bool {
        if let VaultFetchState::Loaded { data, .. } = &mut self.fetch_state {
            // Find the character with this pet
            for character in data.characters.iter_mut() {
                if character.seasonal == seasonal {
                    if let Some(ref mut pet) = character.pet {
                        if pet.instance_id == pet_instance_id {
                            // Ensure inventory is large enough
                            while pet.inventory.len() <= slot {
                                pet.inventory.push(PetInventoryItem::empty());
                            }
                            // Update the slot
                            pet.inventory[slot] = PetInventoryItem {
                                item_id,
                                unique_id: None,
                                stack_count,
                            };
                            // Keep a live-created placeholder pet visible (widgets
                            // hide pets with inventory_slots == 0); only grow for a
                            // real item so an empty update can't over-expand.
                            if item_id != -1 && pet.inventory_slots < (slot as i32 + 1) {
                                pet.inventory_slots = slot as i32 + 1;
                            }
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// Set the active pet for a character (real-time, from ActivePetUpdate).
    /// Updates the matching character's pet instance id so subsequent live
    /// inventory updates land on the correct pet. Creates a minimal pet if the
    /// character has none. Returns true if a matching character was found.
    pub fn set_active_pet(&mut self, char_id: i32, instance_id: i32) -> bool {
        if let VaultFetchState::Loaded { data, .. } = &mut self.fetch_state {
            for character in data.characters.iter_mut() {
                if character.char_id == char_id {
                    // Replace the pet outright when the instance changes so the
                    // previous pet's name/skin/inventory don't linger under the
                    // newly equipped pet. Keep it when the id is unchanged.
                    let same = matches!(character.pet, Some(ref p) if p.instance_id == instance_id);
                    if !same {
                        character.pet = Some(Pet {
                            instance_id,
                            ..Default::default()
                        });
                    }
                    return true;
                }
            }
        }
        false
    }

    /// Load cached account data from disk at the injected functional cache path.
    fn load_cached_data(path: &std::path::Path) -> Option<(AccountData, SystemTime)> {
        if !path.exists() {
            return None;
        }

        let xml = std::fs::read_to_string(path).ok()?;
        let metadata = std::fs::metadata(path).ok()?;
        let cached_at = metadata.modified().ok()?;

        match parse_account_data(&xml) {
            Ok(data) => Some((data, cached_at)),
            Err(e) => {
                tracing::warn!("[VAULT] Failed to parse cached data: {}", e);
                None
            }
        }
    }

    /// Save account data XML to the functional `char_list.xml` cache.
    fn save_cache(&self, xml: &str) {
        let path = &self.char_list_cache_path;
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(path, xml) {
            tracing::warn!("[VAULT] Failed to save cache: {}", e);
        }
    }

    /// Check for completed API fetch.
    pub fn check_api_result(&mut self) {
        if let Some(rx) = &self.api_result_rx {
            match rx.try_recv() {
                Ok(Ok((account_data, xml))) => {
                    tracing::info!(
                        "Loaded account data: {} chars, {} vault chests, {} gifts",
                        account_data.characters.len(),
                        account_data.vault.chest_count(),
                        account_data.gifts.count()
                    );
                    // Save to cache
                    if let Some(xml) = xml {
                        self.save_cache(&xml);
                    }
                    self.fetch_state = VaultFetchState::Loaded {
                        data: Box::new(account_data),
                        cached_at: Some(SystemTime::now()),
                    };
                    self.api_result_rx = None;
                }
                Ok(Err(e)) => {
                    tracing::error!("API error: {}", e);
                    self.fetch_state = VaultFetchState::Error(e);
                    self.api_result_rx = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    // Still loading
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.fetch_state =
                        VaultFetchState::Error("API thread disconnected".to_string());
                    self.api_result_rx = None;
                }
            }
        }
    }

    /// Start fetching account data from the API.
    pub fn fetch_account_data(
        &mut self,
        access_token: &str,
        captured_at: Option<chrono::DateTime<chrono::Utc>>,
    ) {
        let token = access_token.to_string();
        let (tx, rx) = mpsc::channel();

        self.fetch_state = VaultFetchState::Loading;
        self.api_result_rx = Some(rx);

        thread::spawn(move || {
            let client = RotmgApiClient::new(token);
            match client.get_char_list() {
                Ok(xml) => {
                    if xml.contains("<Error>Account in use</Error>") {
                        let _ = tx.send(Err(
                            "Account in use - close the game to fetch vault data.".to_string(),
                        ));
                        return;
                    }
                    if xml.contains("<Error>") {
                        let _ = tx.send(Err("Server returned an error".to_string()));
                        return;
                    }
                    let xml_clone = xml.clone();
                    let result = parse_account_data(&xml)
                        .map(|data| (data, Some(xml_clone)))
                        .map_err(|e| e.to_string());
                    let _ = tx.send(result);
                }
                Err(e) => {
                    let msg = match &e {
                        realmhound_core::api::ApiError::Auth(_) => {
                            realmhound_core::api::token_expiry_message(captured_at)
                        }
                        other => other.to_string(),
                    };
                    let _ = tx.send(Err(msg));
                }
            }
        });
    }

    /// On a fresh token: drop any in-flight old-token request and clear errors.
    pub fn reset_on_new_token(&mut self) {
        self.api_result_rx = None;
        if matches!(
            self.fetch_state,
            VaultFetchState::Error(_) | VaultFetchState::Loading
        ) {
            self.fetch_state = VaultFetchState::NoToken;
        }
    }

    /// The current fetch error message, if the last fetch failed.
    pub fn fetch_error(&self) -> Option<&str> {
        match &self.fetch_state {
            VaultFetchState::Error(msg) => Some(msg.as_str()),
            _ => None,
        }
    }

    /// Render the vault panel.
    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        access_token: &Option<String>,
        token_captured_at: Option<chrono::DateTime<chrono::Utc>>,
        sprite_renderer: &mut SpriteRenderer,
        character_cache: Option<&CharacterCache>,
        shadcn: &Shadcn,
        api_refresh_allowed: bool,
    ) {
        self.token_captured_at = token_captured_at;
        self.check_api_result();

        // Update state if token became available and we have no data
        if access_token.is_some() && matches!(self.fetch_state, VaultFetchState::NoToken) {
            self.fetch_state = VaultFetchState::Loaded {
                data: Box::new(AccountData::default()),
                cached_at: None,
            };
        }

        match &self.fetch_state {
            VaultFetchState::NoToken => {
                self.render_no_token(ui, access_token, shadcn, api_refresh_allowed);
            }
            VaultFetchState::Loading => {
                self.render_loading(ui);
            }
            VaultFetchState::Loaded { data, cached_at } => {
                let data = data.clone();
                let cached_at = *cached_at;
                self.render_storage(
                    ui,
                    &data,
                    access_token,
                    cached_at,
                    sprite_renderer,
                    character_cache,
                    shadcn,
                    api_refresh_allowed,
                );
            }
            VaultFetchState::Error(msg) => {
                let msg = msg.clone();
                self.render_error(ui, &msg, access_token, shadcn, api_refresh_allowed);
            }
        }
    }

    fn render_no_token(
        &mut self,
        ui: &mut egui::Ui,
        access_token: &Option<String>,
        shadcn: &Shadcn,
        api_refresh_allowed: bool,
    ) {
        ui.vertical_centered(|ui| {
            ui.add_space(50.0);
            ui.heading("Vault & Storage");
            ui.add_space(20.0);

            if access_token.is_some() {
                ui.label(
                    RichText::new("✅ Access token captured!")
                        .color(Color32::from_rgb(100, 255, 100)),
                );
                ui.add_space(15.0);

                if Self::vault_fetch_button(ui, shadcn, api_refresh_allowed) {
                    if let Some(token) = access_token {
                        self.fetch_account_data(token, self.token_captured_at);
                    }
                }
            } else {
                ui.label(
                    RichText::new("⏳ Waiting for access token...")
                        .color(Color32::from_rgb(200, 200, 100)),
                );
                ui.add_space(10.0);
                ui.label("Start capture and log into RotMG to capture the access token.");
            }
        });
    }

    fn render_loading(&self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(100.0);
            ui.spinner();
            ui.add_space(10.0);
            ui.label("Fetching vault data from RotMG servers...");
        });
    }

    fn render_error(
        &mut self,
        ui: &mut egui::Ui,
        message: &str,
        access_token: &Option<String>,
        shadcn: &Shadcn,
        api_refresh_allowed: bool,
    ) {
        ui.vertical_centered(|ui| {
            ui.add_space(50.0);
            ui.label(RichText::new("❌ Error").color(Color32::from_rgb(255, 100, 100)));
            ui.add_space(10.0);
            ui.label(message);
            ui.add_space(20.0);

            if Self::vault_fetch_button_labeled(ui, shadcn, "🔄 Retry", api_refresh_allowed) {
                if let Some(token) = access_token {
                    self.fetch_account_data(token, self.token_captured_at);
                }
            }

            ui.add_space(10.0);
            if shadcn.btn(ui, "Clear Error").clicked() {
                self.fetch_state = VaultFetchState::NoToken;
            }
        });
    }

    /// A gated "Fetch Vault Data" button. When account API refreshes are not
    /// permitted (the main client hasn't been seen today), the button is
    /// disabled and shown with an innocuous expired-token hint instead of the
    /// real reason (ensuring the game client makes the day's first server
    /// contact). Returns true when the
    /// enabled button is clicked.
    fn vault_fetch_button(ui: &mut egui::Ui, shadcn: &Shadcn, api_refresh_allowed: bool) -> bool {
        Self::vault_fetch_button_labeled(ui, shadcn, "📦 Fetch Vault Data", api_refresh_allowed)
    }

    fn vault_fetch_button_labeled(
        ui: &mut egui::Ui,
        shadcn: &Shadcn,
        label: &str,
        api_refresh_allowed: bool,
    ) -> bool {
        if api_refresh_allowed {
            return shadcn.btn(ui, label).clicked();
        }
        let resp = ui.add_enabled_ui(false, |ui| shadcn.btn(ui, label)).inner;
        resp.disabled_hover_tip("Your access token has expired. Restart the game to refresh it.");
        false
    }

    fn render_storage(
        &mut self,
        ui: &mut egui::Ui,
        data: &AccountData,
        access_token: &Option<String>,
        cached_at: Option<SystemTime>,
        sprite_renderer: &mut SpriteRenderer,
        character_cache: Option<&CharacterCache>,
        shadcn: &Shadcn,
        api_refresh_allowed: bool,
    ) {
        // Check if we have any data (API or live)
        let has_api_data = data.vault.chest_count() > 0
            || data.gifts.count() > 0
            || data.potions.slot_count() > 0
            || data.materials.chest_count() > 0;

        let has_live_data = self
            .live_vault_data
            .as_ref()
            .map(|d| d.has_any_data())
            .unwrap_or(false);

        if !has_api_data && !has_live_data {
            ui.vertical_centered(|ui| {
                ui.add_space(50.0);
                ui.heading("Vault & Storage");
                ui.add_space(20.0);
                ui.label(
                    RichText::new("✅ Access token available")
                        .color(Color32::from_rgb(100, 255, 100)),
                );
                ui.add_space(15.0);

                if Self::vault_fetch_button(ui, shadcn, api_refresh_allowed) {
                    if let Some(token) = access_token {
                        self.fetch_account_data(token, self.token_captured_at);
                    }
                }

                ui.add_space(20.0);
                ui.label(
                    RichText::new("📡 Or enter your vault in-game to capture live data.")
                        .color(Color32::from_rgb(150, 200, 255))
                        .small(),
                );
                ui.add_space(10.0);
                ui.label(
                    RichText::new("⚠️ Note: Vault API is blocked while the game is running.")
                        .color(Color32::from_rgb(255, 200, 100))
                        .small(),
                );
            });
            return;
        }

        // Header
        shadcn.header_band(ui, shadcn.secondary_header_fill(), |ui| {
            // Top-level tabs: Seasonal | Regular (with icons)
            shadcn.band_row(ui, |ui| {
                // Calculate item counts for each vault type
                let regular_count = self.get_total_items_for_vault(VaultType::Regular, data);
                let seasonal_count = self.get_total_items_for_vault(VaultType::Seasonal, data);

                // Seasonal tab with icon
                let seasonal_label = format!("Seasonal ({})", seasonal_count);
                let seasonal_icon = Some(get_vault_top_tab_icon(true));
                if sprite_renderer
                    .render_icon_button(
                        ui,
                        seasonal_icon,
                        &seasonal_label,
                        self.active_top_tab == VaultTopTab::Seasonal,
                        Some(Color32::WHITE),
                        None,
                    )
                    .clicked()
                {
                    self.active_top_tab = VaultTopTab::Seasonal;
                }

                // Regular tab with icon
                let regular_label = format!("Regular ({})", regular_count);
                let regular_icon = Some(get_vault_top_tab_icon(false));
                if sprite_renderer
                    .render_icon_button(
                        ui,
                        regular_icon,
                        &regular_label,
                        self.active_top_tab == VaultTopTab::Regular,
                        Some(Color32::WHITE),
                        None,
                    )
                    .clicked()
                {
                    self.active_top_tab = VaultTopTab::Regular;
                }
            });
        });

        match self.active_top_tab {
            VaultTopTab::Regular => self.render_vault_view(
                ui,
                VaultType::Regular,
                data,
                cached_at,
                sprite_renderer,
                character_cache,
                shadcn,
            ),
            VaultTopTab::Seasonal => self.render_vault_view(
                ui,
                VaultType::Seasonal,
                data,
                cached_at,
                sprite_renderer,
                character_cache,
                shadcn,
            ),
        }
    }

    /// Get total item count for a vault type (combining live + API data).
    fn get_total_items_for_vault(&self, vault_type: VaultType, api_data: &AccountData) -> usize {
        // Live data takes precedence if available for this vault type
        if let Some(live) = &self.live_vault_data {
            let storage = live.get(vault_type);
            if storage.has_data() {
                return storage.vault_item_count()
                    + storage.gift_item_count()
                    + storage.material_item_count()
                    + storage.potion_item_count()
                    + storage.spoils_item_count();
            }
        }

        // Fall back to API data (only for regular vault)
        if vault_type == VaultType::Regular {
            api_data.vault.total_items()
                + api_data.gifts.count()
                + api_data.materials.total_items()
                + api_data.potions.item_count()
        } else {
            0
        }
    }

    /// Get the data source for a vault type.
    fn get_data_source(&self, vault_type: VaultType, api_data: &AccountData) -> DataSource {
        if let Some(live) = &self.live_vault_data {
            let storage = live.get(vault_type);
            if storage.has_data() {
                return if self.live_vault_from_cache {
                    DataSource::Cached
                } else {
                    DataSource::Live
                };
            }
        }

        // Check API data (only applicable for regular vault)
        if vault_type == VaultType::Regular && api_data.vault.chest_count() > 0 {
            DataSource::Api
        } else {
            DataSource::None
        }
    }

    /// Render a sub-tab button with icon and label.
    fn render_subtab_button(
        ui: &mut egui::Ui,
        current_tab: &mut VaultSubTab,
        tab: VaultSubTab,
        label: &str,
        is_seasonal: bool,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        let is_selected = *current_tab == tab;
        let icon = get_vault_subtab_icon(tab, is_seasonal);
        if sprite_renderer
            .render_icon_button(ui, icon, label, is_selected, None, None)
            .clicked()
        {
            *current_tab = tab;
        }
    }

    /// Render a specific vault type (regular or seasonal).
    fn render_vault_view(
        &mut self,
        ui: &mut egui::Ui,
        vault_type: VaultType,
        api_data: &AccountData,
        _api_cached_at: Option<SystemTime>,
        sprite_renderer: &mut SpriteRenderer,
        character_cache: Option<&CharacterCache>,
        shadcn: &Shadcn,
    ) {
        let source = self.get_data_source(vault_type, api_data);
        let live_storage = self
            .live_vault_data
            .as_ref()
            .map(|d| d.get(vault_type).clone());
        let has_live = live_storage.as_ref().map(|s| s.has_data()).unwrap_or(false);

        // Check if we have data for this vault type (including pets from CharacterCache or API)
        let has_pet = !Self::get_pets_from_cache(character_cache, vault_type).is_empty()
            || !Self::get_pets_from_api(api_data, vault_type).is_empty();
        let has_any_data = match source {
            DataSource::None => has_pet, // Pet data counts as data
            _ => true,
        };

        if !has_any_data {
            ui.vertical_centered(|ui| {
                ui.add_space(30.0);
                ui.label(
                    RichText::new("📦 No data available")
                        .color(Color32::GRAY)
                );
                ui.add_space(10.0);
                let msg = match vault_type {
                    VaultType::Regular => "Enter your vault in-game to capture live data,\nor use the API button above.",
                    VaultType::Seasonal => "Enter your vault with a seasonal character\nto capture live data.",
                };
                ui.label(
                    RichText::new(msg)
                        .small()
                        .color(Color32::GRAY)
                );
            });
            return;
        }

        // Get sub-tab reference based on vault type
        let sub_tab = match vault_type {
            VaultType::Regular => &mut self.regular_sub_tab,
            VaultType::Seasonal => &mut self.seasonal_sub_tab,
        };

        // Sub-tabs with item counts (same secondary band as the top tabs)
        shadcn.header_band_stacked_divided(ui, shadcn.secondary_header_fill(), |ui| {
            shadcn.band_row(ui, |ui| {
                // Calculate counts
                let (
                    vault_count,
                    vault_slots,
                    gift_count,
                    mat_count,
                    mat_slots,
                    pot_count,
                    pot_slots,
                    spoils_count,
                ) = if has_live {
                    let s = live_storage.as_ref().unwrap();
                    (
                        s.vault_item_count(),
                        s.vault_items.len(),
                        s.gift_item_count(),
                        s.material_item_count(),
                        s.material_items.len(),
                        s.potion_item_count(),
                        s.potion_items.len(),
                        s.spoils_item_count(),
                    )
                } else {
                    // API data (only for regular)
                    (
                        api_data.vault.total_items(),
                        api_data.vault.chest_count() * 8,
                        api_data.gifts.count(),
                        api_data.materials.total_items(),
                        api_data.materials.chest_count() * 8,
                        api_data.potions.item_count(),
                        api_data.potions.slot_count(),
                        0,
                    )
                };

                let is_seasonal = vault_type == VaultType::Seasonal;
                Self::render_subtab_button(
                    ui,
                    sub_tab,
                    VaultSubTab::Vault,
                    &format!("Vault ({}/{})", vault_count, vault_slots),
                    is_seasonal,
                    sprite_renderer,
                );
                Self::render_subtab_button(
                    ui,
                    sub_tab,
                    VaultSubTab::Gifts,
                    &format!("Gifts ({})", gift_count),
                    is_seasonal,
                    sprite_renderer,
                );
                Self::render_subtab_button(
                    ui,
                    sub_tab,
                    VaultSubTab::Storage,
                    &format!(
                        "Materials & Potions ({}/{})",
                        mat_count + pot_count,
                        mat_slots + pot_slots
                    ),
                    is_seasonal,
                    sprite_renderer,
                );

                // Spoils tab only for regular vault
                if vault_type == VaultType::Regular && spoils_count > 0 {
                    Self::render_subtab_button(
                        ui,
                        sub_tab,
                        VaultSubTab::Spoils,
                        &format!("Spoils ({})", spoils_count),
                        is_seasonal,
                        sprite_renderer,
                    );
                }
            });
        });

        // Render the selected sub-tab
        match *sub_tab {
            VaultSubTab::Vault => Self::render_vault_items(
                ui,
                vault_type,
                api_data,
                has_live,
                &live_storage,
                sprite_renderer,
            ),
            VaultSubTab::Gifts => Self::render_gift_items(
                ui,
                vault_type,
                api_data,
                has_live,
                &live_storage,
                sprite_renderer,
            ),
            VaultSubTab::Storage => Self::render_storage_items(
                ui,
                vault_type,
                api_data,
                has_live,
                &live_storage,
                sprite_renderer,
                character_cache,
            ),
            VaultSubTab::Spoils => Self::render_spoils_items(ui, &live_storage, sprite_renderer),
        }
    }

    /// Render vault items grid.
    fn render_vault_items(
        ui: &mut egui::Ui,
        vault_type: VaultType,
        api_data: &AccountData,
        has_live: bool,
        live_storage: &Option<LiveVaultStorage>,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        if has_live {
            let items = &live_storage.as_ref().unwrap().vault_items;
            if items.is_empty() {
                ui.label("No vault data captured yet.");
                return;
            }
            Self::render_live_item_grid(ui, "vault_grid", items, sprite_renderer);
        } else if vault_type == VaultType::Regular {
            // Fall back to API data
            if api_data.vault.chest_count() == 0 {
                ui.label("No vault data available.");
                return;
            }
            let all_items: Vec<&StorageItem> = api_data
                .vault
                .chests
                .iter()
                .flat_map(|chest| chest.items.iter())
                .collect();
            Self::render_api_item_grid(ui, "vault_grid", &all_items, sprite_renderer);
        } else {
            ui.label("No vault data captured yet.");
        }
    }

    /// Render gift items grid.
    fn render_gift_items(
        ui: &mut egui::Ui,
        vault_type: VaultType,
        api_data: &AccountData,
        has_live: bool,
        live_storage: &Option<LiveVaultStorage>,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        if has_live {
            let items = &live_storage.as_ref().unwrap().gift_items;
            if items.is_empty() {
                ui.label("No gifts captured yet.");
                return;
            }
            Self::render_live_item_grid(ui, "gifts_grid", items, sprite_renderer);
        } else if vault_type == VaultType::Regular {
            if api_data.gifts.count() == 0 {
                ui.label("No gifts available.");
                return;
            }
            Self::render_api_item_grid(
                ui,
                "gifts_grid",
                &api_data.gifts.items.iter().collect::<Vec<_>>(),
                sprite_renderer,
            );
        } else {
            ui.label("No gifts captured yet.");
        }
    }

    /// Render materials and potions.
    fn render_storage_items(
        ui: &mut egui::Ui,
        vault_type: VaultType,
        api_data: &AccountData,
        has_live: bool,
        live_storage: &Option<LiveVaultStorage>,
        sprite_renderer: &mut SpriteRenderer,
        character_cache: Option<&CharacterCache>,
    ) {
        let nav = super::scroll_nav::ScrollNav::read(ui, "vault_storage_scroll");
        let mut area = ScrollArea::vertical().auto_shrink([false, false]);
        area = nav.apply(area);
        let out = area.show(ui, |ui| {
            if has_live {
                let storage = live_storage.as_ref().unwrap();

                // Materials section
                if !storage.material_items.is_empty() {
                    Self::render_storage_section_header(
                        ui,
                        sprite_renderer,
                        get_vault_materials_icon(),
                        &format!(
                            "Materials ({}/{})",
                            storage.material_item_count(),
                            storage.material_items.len()
                        ),
                    );
                    ui.add_space(5.0);
                    Self::render_live_item_grid_inline(
                        ui,
                        "materials_grid",
                        &storage.material_items,
                        sprite_renderer,
                    );
                    ui.add_space(15.0);
                }

                // Potions section
                if !storage.potion_items.is_empty() {
                    Self::render_storage_section_header(
                        ui,
                        sprite_renderer,
                        get_vault_potions_icon(),
                        &format!(
                            "Potions ({}/{})",
                            storage.potion_item_count(),
                            storage.potion_items.len()
                        ),
                    );
                    ui.add_space(5.0);
                    Self::render_live_item_grid_inline(
                        ui,
                        "potions_grid",
                        &storage.potion_items,
                        sprite_renderer,
                    );
                    ui.add_space(15.0);
                }

                // Pet inventory section - render all pets with inventory
                Self::render_pet_inventories(
                    ui,
                    character_cache,
                    api_data,
                    vault_type,
                    sprite_renderer,
                );

                if storage.material_items.is_empty() && storage.potion_items.is_empty() {
                    ui.label("No storage data captured yet.");
                }
            } else if vault_type == VaultType::Regular {
                // API data
                if api_data.materials.chest_count() > 0 {
                    let material_slots = api_data.materials.chest_count() * 8;
                    Self::render_storage_section_header(
                        ui,
                        sprite_renderer,
                        get_vault_materials_icon(),
                        &format!(
                            "Materials ({}/{})",
                            api_data.materials.total_items(),
                            material_slots
                        ),
                    );
                    ui.add_space(5.0);

                    let all_materials: Vec<&StorageItem> = api_data
                        .materials
                        .chests
                        .iter()
                        .flat_map(|chest| chest.items.iter())
                        .collect();
                    Self::render_api_item_grid_inline(
                        ui,
                        "materials_grid",
                        &all_materials,
                        sprite_renderer,
                    );
                    ui.add_space(15.0);
                }

                if api_data.potions.slot_count() > 0 {
                    Self::render_storage_section_header(
                        ui,
                        sprite_renderer,
                        get_vault_potions_icon(),
                        &format!(
                            "Potions ({}/{})",
                            api_data.potions.item_count(),
                            api_data.potions.slot_count()
                        ),
                    );
                    ui.add_space(5.0);
                    Self::render_api_item_grid_inline(
                        ui,
                        "potions_grid",
                        &api_data.potions.items.iter().collect::<Vec<_>>(),
                        sprite_renderer,
                    );
                    ui.add_space(15.0);
                }

                // Pet inventory section - render all pets with inventory
                Self::render_pet_inventories(
                    ui,
                    character_cache,
                    api_data,
                    vault_type,
                    sprite_renderer,
                );

                if api_data.materials.chest_count() == 0 && api_data.potions.slot_count() == 0 {
                    ui.label("No storage data available.");
                }
            } else {
                // Seasonal vault without live data - show pets if available
                let rendered_pets = Self::render_pet_inventories(
                    ui,
                    character_cache,
                    api_data,
                    vault_type,
                    sprite_renderer,
                );

                if !rendered_pets {
                    ui.label("No storage data captured yet.");
                }
            }
        });
        nav.store(ui, &out);
    }

    /// Render seasonal spoils items (only in regular vault).
    fn render_spoils_items(
        ui: &mut egui::Ui,
        live_storage: &Option<LiveVaultStorage>,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        if let Some(storage) = live_storage {
            if storage.spoils_items.is_empty() {
                ui.label("No seasonal spoils data captured yet.");
                return;
            }

            ui.label(RichText::new("Seasonal Spoils Chest").color(Color32::WHITE));
            ui.label(
                RichText::new("At the end of each season the contents of your Seasonal Vault, Seasonal Materials Chest and Seasonal Pet Inventories will be transfered here")
                    .small()
                    .color(Color32::GRAY)
            );
            ui.add_space(5.0);

            Self::render_live_item_grid(ui, "spoils_grid", &storage.spoils_items, sprite_renderer);
        } else {
            ui.label("No seasonal spoils data captured yet.");
        }
    }

    /// Get all pets from CharacterCache for the specified vault type.
    fn get_pets_from_cache(
        cache: Option<&CharacterCache>,
        vault_type: VaultType,
    ) -> Vec<&CachedPet> {
        let Some(cache) = cache else {
            return Vec::new();
        };
        let pets = match vault_type {
            VaultType::Seasonal => &cache.seasonal_pets,
            VaultType::Regular => &cache.regular_pets,
        };
        pets.values().collect()
    }

    /// Get all pets from API data for the specified vault type.
    fn get_pets_from_api(api_data: &AccountData, vault_type: VaultType) -> Vec<&Pet> {
        let is_seasonal = vault_type == VaultType::Seasonal;
        api_data
            .characters
            .iter()
            .filter(|c| c.seasonal == is_seasonal)
            .filter_map(|c| c.pet.as_ref())
            .collect()
    }

    /// Render all pet inventories for the vault type.
    /// Uses CharacterCache if available, falls back to API for pets not in cache.
    /// Section header for the Materials & Potions tab: a sprite icon (matching
    /// the Treasury section headers) followed by the label text.
    fn render_storage_section_header(
        ui: &mut egui::Ui,
        sprite_renderer: &mut SpriteRenderer,
        icon: TabIconSprite,
        text: &str,
    ) {
        ui.horizontal(|ui| {
            let size = 20.0;
            let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
            sprite_renderer.render_icon(ui, Some(icon), rect);
            ui.add_space(4.0);
            ui.label(RichText::new(text).strong());
        });
    }

    fn render_pet_inventories(
        ui: &mut egui::Ui,
        character_cache: Option<&CharacterCache>,
        api_data: &AccountData,
        vault_type: VaultType,
        sprite_renderer: &mut SpriteRenderer,
    ) -> bool {
        let cached_pets = Self::get_pets_from_cache(character_cache, vault_type);
        let cached_ids: std::collections::HashSet<i32> =
            cached_pets.iter().map(|p| p.instance_id).collect();

        // Collect all pets to render (cache first, then API fallbacks)
        let mut all_pets: Vec<CachedPet> = cached_pets.iter().map(|p| (*p).clone()).collect();
        for pet in Self::get_pets_from_api(api_data, vault_type) {
            if !cached_ids.contains(&pet.instance_id) {
                all_pets.push(CachedPet::from(pet));
            }
        }

        // Only show pets whose inventory is actually unlocked
        all_pets.retain(|p| p.has_inventory());

        // Render pets horizontally, wrapping to next row
        if all_pets.is_empty() {
            return false;
        }
        Self::render_storage_section_header(
            ui,
            sprite_renderer,
            get_vault_pet_inventories_icon(),
            &format!("Pet Inventories ({})", all_pets.len()),
        );
        ui.add_space(5.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
            for pet in &all_pets {
                Self::render_cached_pet_inventory(ui, pet, sprite_renderer);
            }
        });
        true
    }

    /// Render pet inventory section for a CachedPet (from CharacterCache).
    fn render_cached_pet_inventory(
        ui: &mut egui::Ui,
        pet: &CachedPet,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        // Match character card constants for consistency
        const SLOT_SIZE: f32 = 38.0;
        const SLOT_SPACING: f32 = 4.0;
        const CARD_PADDING: f32 = 8.0;
        const CARD_CONTENT_WIDTH: f32 = 4.0 * SLOT_SIZE + 3.0 * SLOT_SPACING; // 164.0

        // Pet card frame
        let fill = crate::ui_colors::card_fill(ui.visuals());
        let stroke_color = crate::ui_colors::card_stroke(ui.visuals());

        ui.push_id(pet.instance_id, |ui| {
            egui::Frame::NONE
                .fill(fill)
                .stroke(egui::Stroke::new(1.0_f32, stroke_color))
                .corner_radius(8.0)
                .inner_margin(CARD_PADDING)
                .show(ui, |ui| {
                    // Force consistent spacing inside the card
                    ui.spacing_mut().item_spacing = egui::vec2(SLOT_SPACING, SLOT_SPACING);

                    ui.set_min_width(CARD_CONTENT_WIDTH);
                    ui.set_max_width(CARD_CONTENT_WIDTH);

                    ui.vertical(|ui| {
                        // Pet header with sprite, name, and rarity
                        ui.horizontal(|ui| {
                            // Pet sprite (using skin ID as item lookup)
                            if pet.skin > 0 {
                                sprite_renderer.render_item_tile(ui, pet.skin, None, &[]);
                                ui.add_space(4.0);
                            }

                            ui.vertical(|ui| {
                                // The stored `name` is the stale original-hatch
                                // family (e.g. "Blue Ant"); a re-skinned pet's real
                                // display name is the skin/type sprite's name (e.g.
                                // "Red Heart"), so resolve it the same way the
                                // sprite does, falling back to the stored name.
                                let sprite_id = if pet.skin > 0 { pet.skin } else { pet.pet_type };
                                let pet_name = get_asset_manager()
                                    .object_name(sprite_id)
                                    .filter(|n| !n.is_empty())
                                    .unwrap_or_else(|| pet.name.clone());
                                ui.label(RichText::new(&pet_name).strong());
                                ui.label(
                                    RichText::new(pet.rarity_name())
                                        .small()
                                        .color(Self::rarity_color(pet.rarity)),
                                );
                            });
                        });

                        // Pet abilities
                        if !pet.abilities.is_empty() {
                            ui.add_space(4.0);
                            let abilities_str: Vec<String> = pet
                                .abilities
                                .iter()
                                .map(|a| format!("{} {}", a.ability_name(), a.power))
                                .collect();
                            ui.label(
                                RichText::new(abilities_str.join(" | "))
                                    .small()
                                    .color(Color32::from_rgb(150, 150, 150)),
                            );
                        }

                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(4.0);

                        // Pet inventory (8 slots in 2 rows of 4)
                        let available_slots = pet.inventory_slots.max(0) as usize;

                        ui.vertical(|ui| {
                            // First row: slots 0-3
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = SLOT_SPACING;
                                for i in 0..4 {
                                    Self::render_cached_pet_slot(
                                        ui,
                                        pet,
                                        i,
                                        available_slots,
                                        sprite_renderer,
                                    );
                                }
                            });

                            // Second row: slots 4-7
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = SLOT_SPACING;
                                for i in 4..8 {
                                    Self::render_cached_pet_slot(
                                        ui,
                                        pet,
                                        i,
                                        available_slots,
                                        sprite_renderer,
                                    );
                                }
                            });
                        });
                    });
                });
        });
    }

    /// Render a single pet inventory slot for a CachedPet.
    fn render_cached_pet_slot(
        ui: &mut egui::Ui,
        pet: &CachedPet,
        slot_index: usize,
        available_slots: usize,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        if slot_index < available_slots {
            // Slot is available
            if let Some(item) = pet.inventory.get(slot_index) {
                if item.item_id < 0 {
                    sprite_renderer.render_empty_slot(ui, 38.0);
                } else {
                    sprite_renderer.render_item_tile(ui, item.item_id, None, &[]);
                }
            } else {
                sprite_renderer.render_empty_slot(ui, 38.0);
            }
        } else {
            // Slot is unavailable (locked)
            sprite_renderer.render_unavailable_slot(ui, 38.0);
        }
    }

    /// Get color for pet rarity.
    fn rarity_color(rarity: i32) -> Color32 {
        match rarity {
            0 => Color32::from_rgb(180, 180, 180), // Common - gray
            1 => Color32::from_rgb(100, 200, 100), // Uncommon - green
            2 => Color32::from_rgb(100, 150, 255), // Rare - blue
            3 => Color32::from_rgb(200, 100, 255), // Legendary - purple
            4 => Color32::from_rgb(255, 200, 50),  // Divine - gold
            _ => Color32::GRAY,
        }
    }

    /// Calculate multi-column layout for item grids.
    ///
    /// Returns a vec of item counts per column, following these rules:
    /// 1. If everything fits in one column, use one column.
    /// 2. If two columns can fit everything on screen without scrolling,
    ///    fill the first column to screen height, remainder in the second.
    /// 3. Otherwise, spread items evenly across as many columns as fit
    ///    on screen. Vertical scroll handles any overflow.
    fn calculate_column_layout(
        total_items: usize,
        available_width: f32,
        available_height: f32,
    ) -> Vec<usize> {
        let cell_size = 38.0 + 8.0; // item_size + grid spacing (row pitch)
                                    // Real rendered column footprint, measured from actual output: each of
                                    // the 8 slots occupies ~40pt wide (the 38pt tile plus its 1pt outside
                                    // stroke on each side) with 8pt grid spacing between them, and columns
                                    // are separated by ~22pt (the `add_space(16)` plus the surrounding
                                    // layout spacing). Modeling the true width keeps a partially-visible
                                    // column from being counted as fitting.
        let single_column_width = 8.0 * 40.0 + 7.0 * 8.0;
        let column_gap = 22.0;

        let total_rows = (total_items + 7) / 8;
        if total_rows == 0 {
            return vec![total_items];
        }

        let rows_per_screen = (available_height / cell_size).floor().max(1.0) as usize;
        let max_cols_by_width = ((available_width + column_gap)
            / (single_column_width + column_gap))
            .floor()
            .max(1.0) as usize;

        // Case 1: fits in one column, or screen too narrow for multiple
        if total_rows <= rows_per_screen || max_cols_by_width <= 1 {
            return vec![total_items];
        }

        // Case 2: two columns fit everything on screen without scrolling
        if total_rows <= rows_per_screen * 2 && max_cols_by_width >= 2 {
            let col1_items = (rows_per_screen * 8).min(total_items);
            let col2_items = total_items - col1_items;
            return vec![col1_items, col2_items];
        }

        // Case 3: spread evenly across columns, capped by screen width
        let needed_cols = (total_rows + rows_per_screen - 1) / rows_per_screen;
        let num_cols = needed_cols.min(max_cols_by_width);
        let rows_per_col = (total_rows + num_cols - 1) / num_cols;
        let items_per_col = rows_per_col * 8;

        let mut columns = Vec::new();
        let mut remaining = total_items;
        for _ in 0..num_cols {
            if remaining == 0 {
                break;
            }
            let col_items = remaining.min(items_per_col);
            columns.push(col_items);
            remaining -= col_items;
        }

        columns
    }

    /// Render a grid of live vault items with scroll area and multi-column layout.
    fn render_live_item_grid(
        ui: &mut egui::Ui,
        grid_id: &str,
        items: &[LiveVaultItem],
        sprite_renderer: &mut SpriteRenderer,
    ) {
        let available_width = ui.available_width();
        let available_height = ui.available_height();
        let columns = Self::calculate_column_layout(items.len(), available_width, available_height);

        let nav = super::scroll_nav::ScrollNav::read(ui, grid_id);
        let mut area = ScrollArea::vertical().auto_shrink([false, false]);
        area = nav.apply(area);
        let out = area.show(ui, |ui| {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                let mut offset = 0;
                for (col_idx, &col_item_count) in columns.iter().enumerate() {
                    if col_item_count == 0 {
                        continue;
                    }
                    let end = (offset + col_item_count).min(items.len());
                    let col_items = &items[offset..end];

                    egui::Grid::new(format!("{}_{}", grid_id, col_idx))
                        .num_columns(8)
                        .spacing([8.0, 8.0])
                        .show(ui, |ui| {
                            for (idx, item) in col_items.iter().enumerate() {
                                Self::render_live_item_slot(ui, item, idx, sprite_renderer);
                                if (idx + 1) % 8 == 0 {
                                    ui.end_row();
                                }
                            }
                        });

                    if col_idx < columns.len() - 1 {
                        ui.add_space(16.0);
                    }

                    offset = end;
                }
            });
        });
        nav.store(ui, &out);
    }

    /// Render a grid of live vault items without scroll area.
    fn render_live_item_grid_inline(
        ui: &mut egui::Ui,
        grid_id: &str,
        items: &[LiveVaultItem],
        sprite_renderer: &mut SpriteRenderer,
    ) {
        egui::Grid::new(grid_id)
            .num_columns(8)
            .spacing([8.0, 8.0])
            .show(ui, |ui| {
                for (idx, item) in items.iter().enumerate() {
                    Self::render_live_item_slot(ui, item, idx, sprite_renderer);
                    if (idx + 1) % 8 == 0 {
                        ui.end_row();
                    }
                }
            });
    }

    /// Render a grid of API storage items with scroll area and multi-column layout.
    fn render_api_item_grid(
        ui: &mut egui::Ui,
        grid_id: &str,
        items: &[&StorageItem],
        sprite_renderer: &mut SpriteRenderer,
    ) {
        let available_width = ui.available_width();
        let available_height = ui.available_height();
        let columns = Self::calculate_column_layout(items.len(), available_width, available_height);

        let nav = super::scroll_nav::ScrollNav::read(ui, grid_id);
        let mut area = ScrollArea::vertical().auto_shrink([false, false]);
        area = nav.apply(area);
        let out = area.show(ui, |ui| {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                let mut offset = 0;
                for (col_idx, &col_item_count) in columns.iter().enumerate() {
                    if col_item_count == 0 {
                        continue;
                    }
                    let end = (offset + col_item_count).min(items.len());
                    let col_items = &items[offset..end];

                    egui::Grid::new(format!("{}_{}", grid_id, col_idx))
                        .num_columns(8)
                        .spacing([8.0, 8.0])
                        .show(ui, |ui| {
                            for (idx, item) in col_items.iter().enumerate() {
                                Self::render_item_slot(ui, item, idx, sprite_renderer);
                                if (idx + 1) % 8 == 0 {
                                    ui.end_row();
                                }
                            }
                        });

                    if col_idx < columns.len() - 1 {
                        ui.add_space(16.0);
                    }

                    offset = end;
                }
            });
        });
        nav.store(ui, &out);
    }

    /// Render a grid of API storage items without scroll area.
    fn render_api_item_grid_inline(
        ui: &mut egui::Ui,
        grid_id: &str,
        items: &[&StorageItem],
        sprite_renderer: &mut SpriteRenderer,
    ) {
        egui::Grid::new(grid_id)
            .num_columns(8)
            .spacing([8.0, 8.0])
            .show(ui, |ui| {
                for (idx, item) in items.iter().enumerate() {
                    Self::render_item_slot(ui, item, idx, sprite_renderer);
                    if (idx + 1) % 8 == 0 {
                        ui.end_row();
                    }
                }
            });
    }

    /// Render a single live vault item slot.
    fn render_live_item_slot(
        ui: &mut egui::Ui,
        item: &LiveVaultItem,
        _idx: usize,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        if item.is_empty() {
            sprite_renderer.render_empty_slot(ui, 38.0);
        } else {
            // Use fixed-size array to avoid heap allocation (max 4 enchants)
            let mut enchant_buf: [i32; 4] = [0; 4];
            let enchant_count = item.enchant_ids.len().min(4);
            for (i, &e) in item.enchant_ids.iter().take(4).enumerate() {
                enchant_buf[i] = e as i32;
            }
            sprite_renderer.render_item_tile(ui, item.item_id, None, &enchant_buf[..enchant_count]);
        }
    }

    fn render_item_slot(
        ui: &mut egui::Ui,
        item: &StorageItem,
        _idx: usize,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        if item.is_empty() {
            sprite_renderer.render_empty_slot(ui, 38.0);
        } else {
            // Use fixed-size array to avoid heap allocation (max 4 enchants)
            let enchants = item.enchant_ids();
            let mut enchant_buf: [i32; 4] = [0; 4];
            let enchant_count = enchants.len().min(4);
            for (i, &e) in enchants.iter().take(4).enumerate() {
                enchant_buf[i] = e as i32;
            }
            sprite_renderer.render_item_tile(ui, item.item_id, None, &enchant_buf[..enchant_count]);
        }
    }
}

impl Panel for VaultPanel {
    fn show(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext) -> Vec<AppAction> {
        // Use account_data.characters as the character cache source
        self.render(
            ui,
            ctx.access_token,
            ctx.token_captured_at,
            ctx.sprite_renderer,
            Some(&ctx.account_data.characters),
            ctx.shadcn,
            ctx.api_refresh_allowed,
        );
        vec![]
    }

    fn handle_event(
        &mut self,
        event: &realmhound_core::GameEvent,
        _session: &realmhound_core::GameSession,
    ) -> Vec<AppAction> {
        if let realmhound_core::GameEvent::VaultContentReceived {
            ref packet,
            vault_type,
        } = event
        {
            // Note: VaultContentReceived also updates app-level live_vault_data.
            // That wiring remains in dispatch_event() since the panel receives
            // the data via set_live_vault_data() after the app updates it.
            let _ = (packet, vault_type); // Handled by app-level dispatch
        }
        vec![]
    }
}

#[cfg(test)]
mod path_injection_tests {
    use super::*;

    fn empty_live_vault() -> LiveVaultData {
        LiveVaultData {
            regular: LiveVaultStorage::default(),
            seasonal: LiveVaultStorage::default(),
        }
    }

    #[test]
    fn char_list_functional_cache_writes_to_injected_path() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache").join("char_list.xml");
        let panel = VaultPanel::with_live_vault_data(empty_live_vault(), cache.clone());
        // The functional char_list cache is written regardless of (disabled) API
        // diagnostics, to the injected per-account path.
        panel.save_cache("<Account/>");
        assert!(cache.exists());
        assert_eq!(std::fs::read_to_string(&cache).unwrap(), "<Account/>");
    }

    #[test]
    fn two_accounts_use_isolated_char_list_paths() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a").join("char_list.xml");
        let b = temp.path().join("b").join("char_list.xml");
        let panel_a = VaultPanel::with_live_vault_data(empty_live_vault(), a.clone());
        panel_a.save_cache("<Account/>");
        assert!(a.exists());
        assert!(!b.exists());
        assert_ne!(a, b);
    }
}

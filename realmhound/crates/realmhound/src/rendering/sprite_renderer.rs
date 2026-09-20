//! Sprite rendering for egui using RotMG game assets.
//!
//! This module provides efficient sprite rendering with:
//! - Texture atlas loading (4 PNG spritesheets)
//! - Sprite cropping via UV coordinates (no pixel copies)
//! - Caching of loaded textures
//! - Glow texture generation with pixel-level manipulation
//! - Embedded overlay sprites (shiny, enchant indicators)
//! - Fallback for missing sprites
//!
//! # Performance
//!
//! The sprite system is optimized for rendering thousands of items:
//! - Atlas textures are loaded once and shared across all items
//! - UV coordinates are used for sub-image rendering (GPU efficient)
//! - Glow textures are generated once and cached
//! - Overlay sprites are embedded in the binary (no external files)
//! - Texture handles are cached globally per egui context

use crate::ui_ext::HoverTooltipExt;
use std::collections::HashMap;
use std::sync::mpsc;
use std::thread;

#[cfg(feature = "profiling")]
use std::sync::atomic::{AtomicU32, Ordering};

/// Per-frame sprite draw-call counter (only active with `profiling` feature).
#[cfg(feature = "profiling")]
static SPRITE_DRAW_CALLS: AtomicU32 = AtomicU32::new(0);

/// Per-frame outlined-sprite counter (each = 9 draw calls).
#[cfg(feature = "profiling")]
static OUTLINED_SPRITE_COUNT: AtomicU32 = AtomicU32::new(0);

/// Increment the sprite draw-call counter. Called from painter.image() wrappers.
#[cfg(feature = "profiling")]
#[inline]
fn count_draw_call() {
    SPRITE_DRAW_CALLS.fetch_add(1, Ordering::Relaxed);
}

#[cfg(not(feature = "profiling"))]
#[inline(always)]
fn count_draw_call() {}

#[cfg(feature = "profiling")]
#[inline]
fn count_outlined_sprite() {
    OUTLINED_SPRITE_COUNT.fetch_add(1, Ordering::Relaxed);
}

#[cfg(not(feature = "profiling"))]
#[inline(always)]
fn count_outlined_sprite() {}

/// Lighten a color by adding `amount` to each RGB channel (saturating).
#[inline]
fn lighten_color(c: egui::Color32, amount: u8) -> egui::Color32 {
    egui::Color32::from_rgba_premultiplied(
        c.r().saturating_add(amount),
        c.g().saturating_add(amount),
        c.b().saturating_add(amount),
        c.a(),
    )
}

/// Object ids whose display scale should ignore thin edge protrusions (see
/// [`SpriteRenderer::dense_trim_wh`]). Currently only the Plagued Nest portal
/// (17570), whose 2px corner "feet" otherwise cap its scale a step below the
/// visually identical The Nest portal shown beside it in mission objectives.
fn dense_trim_for_scale(item_id: i32) -> bool {
    matches!(item_id, 17570)
}

/// Reset the per-frame draw counter and return (draw_calls, outlined_sprites).
#[cfg(feature = "profiling")]
pub fn take_draw_call_count() -> (u32, u32) {
    (
        SPRITE_DRAW_CALLS.swap(0, Ordering::Relaxed),
        OUTLINED_SPRITE_COUNT.swap(0, Ordering::Relaxed),
    )
}

#[cfg(not(feature = "profiling"))]
#[inline(always)]
#[allow(dead_code)]
pub fn take_draw_call_count() -> (u32, u32) {
    (0, 0)
}

use eframe::egui::{self, Color32, ColorImage, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use image::{DynamicImage, GenericImageView, ImageReader, Rgba, RgbaImage};
use realmhound_core::assets::{get_asset_manager, RealmEyeDropData};

use crate::rendering::{EmbeddedIcon, ModIconBase, ModTierIcon};
use crate::tab_icons::{TabIconSprite, TAB_ICON_SIZE};

// Embedded overlay sprites (compiled into the binary)
const SHINY_SPRITE: &[u8] = include_bytes!("../../assets/Shiny.png");
const ENCHANT_UNCOMMON_SPRITE: &[u8] = include_bytes!("../../assets/Enchants-Uncommon.png");
const ENCHANT_RARE_SPRITE: &[u8] = include_bytes!("../../assets/Enchants-Rare.png");
const ENCHANT_LEGENDARY_SPRITE: &[u8] = include_bytes!("../../assets/Enchants-Legendary.png");
const ENCHANT_DIVINE_SPRITE: &[u8] = include_bytes!("../../assets/Enchants-Divine.png");

/// Key for cached glow textures: (item_id, glow_color_rgb, glow_size)
/// Note: We generate at native sprite resolution and let GPU scale with nearest-neighbor
type GlowCacheKey = (i32, u32, u8, u32); // (item_id, color, glow_size, target_size)

/// Key for cached character glow textures. Includes direction and dyes so a
/// character's silhouette glow never collides with an item glow or another pose.
/// (sprite_id, direction, tex1, tex2, color, glow_size, target_size)
type CharGlowCacheKey = (i32, i32, u32, u32, u32, u8, u32);

/// Glow colour for the "Lone fighter" secret stat (gold, like a Divine weapon).
pub const LONE_FIGHTER_GLOW: Color32 = Color32::from_rgb(255, 215, 0);
/// Glow colour for the "Last hero standing" secret stat (purple, like a
/// Legendary weapon).
pub const LAST_HERO_GLOW: Color32 = Color32::from_rgb(200, 100, 255);
/// Glow colour for the "Most damage taken" secret stat (red).
pub const MOST_DAMAGE_TAKEN_GLOW: Color32 = Color32::from_rgb(255, 60, 60);

/// Cache key for character dye compositing: (sprite_id, direction, tex1, tex2)
type CharDyeCacheKey = (i32, i32, u32, u32);

/// Cached dye-composited sprite (base + mask + color).
struct DyeCacheEntry {
    texture: TextureHandle,
    image: RgbaImage,
    width: u32,
    height: u32,
}

/// Data for a loaded atlas (sent from background thread).
struct LoadedAtlasData {
    atlas_id: u8,
    image: DynamicImage,
    width: u32,
    height: u32,
}

/// Atlas loading state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtlasLoadState {
    /// Not started
    NotStarted,
    /// Loading in progress
    Loading,
    /// Fully loaded
    Ready,
}

/// Sprite renderer for drawing item sprites in egui.
pub struct SpriteRenderer {
    /// Local texture cache
    textures: HashMap<u8, TextureHandle>,
    /// Atlas dimensions
    atlas_dimensions: HashMap<u8, (u32, u32)>,
    /// Atlas images (loaded lazily)
    atlas_images: HashMap<u8, DynamicImage>,
    /// Atlas loading state
    load_state: AtlasLoadState,
    /// Receiver for loaded atlas data from background thread
    atlas_rx: Option<mpsc::Receiver<LoadedAtlasData>>,
    /// Number of atlases expected to load
    atlases_pending: usize,
    /// Cache for generated glow textures (item_id, color, size) -> TextureHandle
    glow_cache: HashMap<GlowCacheKey, TextureHandle>,
    /// Cache for generated character silhouette glow textures.
    char_glow_cache: HashMap<CharGlowCacheKey, TextureHandle>,
    /// Cache for dye-composited sprites: item_id -> composited texture + image
    dye_cache: HashMap<i32, DyeCacheEntry>,
    /// Cache for character dye-composited sprites: (sprite_id, direction, tex1, tex2) -> texture + image
    char_dye_cache: HashMap<CharDyeCacheKey, DyeCacheEntry>,
    /// Embedded overlay texture: shiny indicator (50x50)
    shiny_texture: Option<TextureHandle>,
    /// Embedded overlay textures: enchant indicators by tier (16x16 each)
    /// Index 0 = 1 enchant (Uncommon), 1 = 2 (Rare), 2 = 3 (Legendary), 3 = 4+ (Divine)
    enchant_textures: [Option<TextureHandle>; 4],
    /// Same enchant gems as [`Self::enchant_textures`] but loaded with LINEAR
    /// filtering, for smooth downscaling when drawn as a small corner badge over
    /// list-scale item icons (NEAREST looks jagged below native 16px).
    enchant_textures_smooth: [Option<TextureHandle>; 4],
    /// Embedded PNG icons (tab icons, stat icons, dungeon-callout icons), keyed
    /// by [`EmbeddedIcon`]. Loaded up front in `load_overlay_sprites`.
    embedded_icons: HashMap<EmbeddedIcon, TextureHandle>,
    /// Tier-specific dungeon-modifier icons, keyed by [`ModTierIcon`].
    /// Loaded up front in `load_overlay_sprites`.
    modifier_icons: HashMap<ModTierIcon, TextureHandle>,
    /// Baked outlined sprites: sprite with 1px black dilation outline composited
    /// at a specific integer display scale. Keyed by (item_id, scale).
    outlined_cache: HashMap<(i32, u32), TextureHandle>,
    /// Baked outlined direction-aware sprites. Keyed by (sprite_id, direction, scale, tex1, tex2).
    outlined_dir_cache: HashMap<(i32, i32, u32, u32, u32), TextureHandle>,
    /// Baked colour-outlined direction-aware sprites (secret-stat glow border).
    /// Keyed by (sprite_id, direction, scale, tex1, tex2, outline_rgb).
    outlined_dir_glow_cache: HashMap<(i32, i32, u32, u32, u32, u32), TextureHandle>,
    /// Baked outlined embedded icons (tab icons). Keyed by (icon, scale).
    outlined_embedded_cache: HashMap<(EmbeddedIcon, u32), TextureHandle>,
    /// Baked outlined sheet sprites (tab icons referenced by sheet+index). Keyed by (sheet, index, scale).
    outlined_sheet_cache: HashMap<(&'static str, i32, u32), TextureHandle>,
    /// Baked outlined full-cell sprites: like [`Self::outlined_cache`] but the
    /// source region is extended below the authored rect to capture art the
    /// sprite's `position` crops off (e.g. exaltation banners). Keyed by
    /// (item_id, scale).
    outlined_full_cache: HashMap<(i32, u32), TextureHandle>,
    /// Baked outlined sprites resized to fit an arbitrary target pixel size:
    /// the native art is smoothly (Triangle) scaled to a fractional target and a
    /// crisp 1px outline is baked at final resolution. Keyed by
    /// (item_id, target_w_px, target_h_px). Used for full-frame portal icons
    /// (e.g. Kogbold Steamworks) that integer scaling can't grow to match
    /// dome-style portals in the same cell.
    outlined_fit_cache: HashMap<(i32, u32, u32), TextureHandle>,
    /// Owned-item rarity breakdown: item_id -> count owned per enchant-slot
    /// rarity (index 0 = Common .. 4 = Divine). Refreshed from account data.
    owned_rarities: HashMap<i32, [u32; 5]>,
    /// When true, item tooltips omit the "Owned rarity" section. Set by views
    /// that show other players' gear (e.g. Combat History), where the app-user's
    /// own collection counts are irrelevant.
    hide_owned_rarity: bool,
    /// Account generation the `owned_rarities` map was built from (skip recompute).
    owned_rarities_generation: u64,
    /// Per-item content-centering offset in native sprite pixels
    /// (`frame_center - content_center`). Sprites are stored in fixed frames
    /// with visible pixels sometimes off-center (e.g. rings); this shifts them
    /// so the visible content centers in its cell, matching the in-game look.
    content_offsets: HashMap<i32, egui::Vec2>,
    /// Per-sprite-region opaque bounding box `(x, y, w, h)` within the atlas,
    /// used to trim fully-transparent padding so padded sprites render tight and
    /// centered. Keyed by the untrimmed atlas region.
    trim_cache: HashMap<(u8, i32, i32, i32, i32), (i32, i32, i32, i32)>,
    /// Full `CollectionIcon` sprite sheet (streamed from the game as a single
    /// PNG), used to draw forge/tooltip category icons by frame index. Loaded
    /// in `load_overlay_sprites`; `None` if the sheet is missing.
    collection_icon_texture: Option<TextureHandle>,
    /// Pixel dimensions of the loaded `CollectionIcon` sheet, for grid math.
    collection_icon_dims: (u32, u32),
    /// Set once a `CollectionIcon.png` decode fails, to avoid reopening and
    /// warning about a corrupt file every frame. Cleared only by a new renderer
    /// (app restart) or re-extraction producing a valid file next launch.
    collection_icon_failed: bool,
    /// How tab/sub-tab headers display their icon and text. Applied from
    /// [`AppearanceSettings`] on startup and whenever the setting changes.
    tab_label_mode: realmhound_core::settings::TabLabelMode,
    /// Whether tab/sub-tab headers show hover tooltips.
    tab_tooltips: bool,
}

impl Default for SpriteRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl SpriteRenderer {
    /// Create a new sprite renderer.
    pub fn new() -> Self {
        Self {
            textures: HashMap::new(),
            atlas_dimensions: HashMap::new(),
            atlas_images: HashMap::new(),
            load_state: AtlasLoadState::NotStarted,
            atlas_rx: None,
            atlases_pending: 0,
            glow_cache: HashMap::new(),
            char_glow_cache: HashMap::new(),
            dye_cache: HashMap::new(),
            char_dye_cache: HashMap::new(),
            shiny_texture: None,
            enchant_textures: [None, None, None, None],
            enchant_textures_smooth: [None, None, None, None],
            embedded_icons: HashMap::new(),
            modifier_icons: HashMap::new(),
            outlined_cache: HashMap::new(),
            outlined_dir_cache: HashMap::new(),
            outlined_dir_glow_cache: HashMap::new(),
            outlined_embedded_cache: HashMap::new(),
            outlined_sheet_cache: HashMap::new(),
            outlined_full_cache: HashMap::new(),
            outlined_fit_cache: HashMap::new(),
            owned_rarities: HashMap::new(),
            hide_owned_rarity: false,
            owned_rarities_generation: u64::MAX,
            content_offsets: HashMap::new(),
            trim_cache: HashMap::new(),
            collection_icon_texture: None,
            collection_icon_dims: (0, 0),
            collection_icon_failed: false,
            tab_label_mode: realmhound_core::settings::TabLabelMode::default(),
            tab_tooltips: true,
        }
    }

    /// Load embedded overlay sprites (shiny and enchant indicators).
    /// Call this once after creating the renderer, before first render.
    pub fn load_overlay_sprites(&mut self, ctx: &egui::Context) {
        // Load shiny overlay (50x50)
        if self.shiny_texture.is_none() {
            if let Ok(image) = image::load_from_memory(SHINY_SPRITE) {
                let rgba = image.to_rgba8();
                let (w, h) = rgba.dimensions();
                let color_image =
                    ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw());
                self.shiny_texture =
                    Some(ctx.load_texture("shiny_overlay", color_image, TextureOptions::NEAREST));
                tracing::debug!("Loaded embedded shiny overlay sprite ({}x{})", w, h);
            } else {
                tracing::warn!("Failed to decode embedded Shiny.png");
            }
        }

        // Load enchant overlays (16x16 each)
        let enchant_sprites = [
            (0, ENCHANT_UNCOMMON_SPRITE, "Enchants-Uncommon"),
            (1, ENCHANT_RARE_SPRITE, "Enchants-Rare"),
            (2, ENCHANT_LEGENDARY_SPRITE, "Enchants-Legendary"),
            (3, ENCHANT_DIVINE_SPRITE, "Enchants-Divine"),
        ];

        for (idx, sprite_data, name) in enchant_sprites {
            if self.enchant_textures[idx].is_none() {
                if let Ok(image) = image::load_from_memory(sprite_data) {
                    let rgba = image.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    let color_image =
                        ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw());
                    self.enchant_textures[idx] = Some(ctx.load_texture(
                        format!("enchant_overlay_{}", idx),
                        color_image.clone(),
                        TextureOptions::NEAREST,
                    ));
                    self.enchant_textures_smooth[idx] = Some(ctx.load_texture(
                        format!("enchant_overlay_smooth_{}", idx),
                        color_image,
                        TextureOptions::LINEAR,
                    ));
                    tracing::debug!("Loaded embedded {} overlay sprite ({}x{})", name, w, h);
                } else {
                    tracing::warn!("Failed to decode embedded {}.png", name);
                }
            }
        }

        // Load all embedded PNG icons (tab icons, stat icons, dungeon-callout
        // grade + mod-type icons) into one cache keyed by EmbeddedIcon.
        for &icon in EmbeddedIcon::ALL {
            if self.embedded_icons.contains_key(&icon) {
                continue;
            }
            match image::load_from_memory(icon.bytes()) {
                Ok(image) => {
                    let rgba = image.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    let color_image =
                        ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw());
                    let texture =
                        ctx.load_texture(icon.texture_name(), color_image, TextureOptions::NEAREST);
                    self.embedded_icons.insert(icon, texture);
                    tracing::debug!("Loaded embedded icon {} ({}x{})", icon.texture_name(), w, h);
                }
                Err(_) => {
                    tracing::warn!("Failed to decode embedded icon {}", icon.texture_name());
                }
            }
        }

        // Load all tier-specific modifier icons (37 bases x 4 tiers).
        for &base in ModIconBase::ALL {
            for tier in 1..=4u8 {
                let Some(icon) = ModTierIcon::new(base, tier) else {
                    continue;
                };
                if self.modifier_icons.contains_key(&icon) {
                    continue;
                }
                match image::load_from_memory(icon.bytes()) {
                    Ok(image) => {
                        let rgba = image.to_rgba8();
                        let (w, h) = rgba.dimensions();
                        let color_image = ColorImage::from_rgba_unmultiplied(
                            [w as usize, h as usize],
                            rgba.as_raw(),
                        );
                        let texture = ctx.load_texture(
                            format!("modifier_{:?}_{}", base, tier),
                            color_image,
                            // Linear gives a clean downscale for these detailed
                            // icons; nearest drops pixels unevenly at the
                            // non-integer render size and looks distorted.
                            TextureOptions::LINEAR,
                        );
                        self.modifier_icons.insert(icon, texture);
                    }
                    Err(_) => {
                        tracing::warn!("Failed to decode modifier icon {:?} tier {}", base, tier);
                    }
                }
            }
        }

        // Load the CollectionIcon sheet (forge/tooltip category icons) from the
        // extracted game asset, if present. Drawn by frame index via
        // `draw_collection_icon`. The `exists()` guard avoids per-frame work
        // while assets are still extracting (missing file is the normal early
        // state, not an error).
        if self.collection_icon_texture.is_none() && !self.collection_icon_failed {
            if let Some(dir) = get_asset_manager().assets_dir() {
                let path = dir.join("sprites").join("CollectionIcon.png");
                if path.exists() {
                    match image::open(&path) {
                        Ok(image) => {
                            let rgba = image.to_rgba8();
                            let (w, h) = rgba.dimensions();
                            let color_image = ColorImage::from_rgba_unmultiplied(
                                [w as usize, h as usize],
                                rgba.as_raw(),
                            );
                            self.collection_icon_texture = Some(ctx.load_texture(
                                "collection_icon_sheet",
                                color_image,
                                TextureOptions::NEAREST,
                            ));
                            self.collection_icon_dims = (w, h);
                            tracing::debug!("Loaded CollectionIcon sheet ({}x{})", w, h);
                        }
                        Err(e) => {
                            tracing::warn!("Failed to load CollectionIcon.png: {}", e);
                            self.collection_icon_failed = true;
                        }
                    }
                }
            }
        }
    }

    /// Start loading all atlas textures in the background.
    /// Call this once, then poll with `poll_atlas_loading` each frame.
    pub fn start_atlas_loading(&mut self) {
        if self.load_state != AtlasLoadState::NotStarted {
            return;
        }

        let assets_dir = match get_asset_manager().assets_dir() {
            Some(dir) => dir,
            None => return,
        };

        let (tx, rx) = mpsc::channel();
        self.atlas_rx = Some(rx);
        self.load_state = AtlasLoadState::Loading;
        self.atlases_pending = 4; // We load 4 atlases

        // Spawn background thread to load all atlases
        thread::spawn(move || {
            let atlas_names = [
                (1u8, "groundTiles"),
                (2u8, "characters"),
                (3u8, "characters_masks"),
                (4u8, "mapObjects"),
            ];

            for (atlas_id, atlas_name) in atlas_names {
                let path = assets_dir
                    .join("sprites")
                    .join(format!("{}.png", atlas_name));

                if !path.exists() {
                    tracing::warn!("Atlas file not found: {:?}", path);
                    continue;
                }

                match ImageReader::open(&path) {
                    Ok(reader) => match reader.decode() {
                        Ok(img) => {
                            let (width, height) = img.dimensions();
                            tracing::info!(
                                "Loaded sprite atlas {}: {}x{}",
                                atlas_name,
                                width,
                                height
                            );
                            let _ = tx.send(LoadedAtlasData {
                                atlas_id,
                                image: img,
                                width,
                                height,
                            });
                        }
                        Err(e) => {
                            tracing::warn!("Failed to decode atlas {}: {}", atlas_name, e);
                        }
                    },
                    Err(e) => {
                        tracing::warn!("Failed to open atlas {}: {}", atlas_name, e);
                    }
                }
            }
        });
    }

    /// Poll for loaded atlas data and create textures.
    /// Call this every frame when load_state is Loading.
    /// Returns true when all atlases are loaded.
    pub fn poll_atlas_loading(&mut self, ctx: &egui::Context) -> bool {
        if self.load_state == AtlasLoadState::Ready {
            return true;
        }

        if self.load_state == AtlasLoadState::NotStarted {
            self.start_atlas_loading();
            return false;
        }

        // Process any loaded atlases from background thread
        if let Some(rx) = &self.atlas_rx {
            while let Ok(data) = rx.try_recv() {
                self.atlas_dimensions
                    .insert(data.atlas_id, (data.width, data.height));

                let rgba = data.image.to_rgba8();
                let color_image = ColorImage::from_rgba_unmultiplied(
                    [data.width as usize, data.height as usize],
                    rgba.as_raw(),
                );

                let texture = ctx.load_texture(
                    format!("atlas_{}", data.atlas_id),
                    color_image,
                    TextureOptions::NEAREST,
                );

                self.textures.insert(data.atlas_id, texture);
                self.atlas_images.insert(data.atlas_id, data.image);

                self.atlases_pending = self.atlases_pending.saturating_sub(1);
            }
        }

        // Check if all atlases are loaded
        if self.atlases_pending == 0 && self.textures.len() >= 2 {
            // At minimum we need characters (2) and mapObjects (4) for vault items
            self.load_state = AtlasLoadState::Ready;
            self.atlas_rx = None;
            return true;
        }

        false
    }

    /// Get the display name for an item ID.
    pub fn item_name(&self, item_id: i32) -> Option<String> {
        get_asset_manager().object_name(item_id)
    }

    /// Check if an item is a shiny variant.
    pub fn is_shiny(&self, item_id: i32) -> bool {
        get_asset_manager().is_shiny(item_id)
    }

    /// Refresh the owned-item rarity breakdown from account data. Counts every
    /// owned copy of each item by its enchant-slot rarity (0 = Common .. 4 =
    /// Divine) across all live and dead characters and both vaults. Skips the
    /// (potentially large) recompute when the account generation is unchanged.
    /// Toggle whether item tooltips include the "Owned rarity" section (the
    /// app-user's own collection counts). Views that display other players' gear
    /// (Combat History) set this false so those counts don't appear.
    pub fn set_hide_owned_rarity(&mut self, hide: bool) {
        self.hide_owned_rarity = hide;
    }

    pub fn update_owned_rarities(&mut self, account_data: &realmhound_core::vault::AccountData) {
        if self.owned_rarities_generation == account_data.generation {
            return;
        }
        self.owned_rarities_generation = account_data.generation;
        self.owned_rarities.clear();

        let mut add = |item_id: i32, enchant_count: usize| {
            if item_id > 0 {
                let slot = enchant_count.min(4);
                self.owned_rarities.entry(item_id).or_insert([0; 5])[slot] += 1;
            }
        };

        for ch in account_data
            .characters
            .characters
            .iter()
            .chain(account_data.characters.get_dead_characters())
        {
            for item in ch
                .equipment
                .iter()
                .chain(ch.inventory.iter())
                .chain(ch.backpack.iter())
                .chain(ch.backpack_ext.iter())
                .chain(ch.belt.iter())
            {
                add(item.item_id, item.enchant_count());
            }
        }
        for vault in [&account_data.regular_vault, &account_data.seasonal_vault] {
            for item in vault
                .vault_items
                .iter()
                .chain(vault.material_items.iter())
                .chain(vault.gift_items.iter())
                .chain(vault.potion_items.iter())
                .chain(vault.spoils_items.iter())
            {
                add(item.item_id, item.enchant_count());
            }
        }
    }

    /// Get the display name for an enchantment type ID.
    pub fn enchant_name(&self, type_id: u16) -> Option<String> {
        get_asset_manager().enchant_name(type_id)
    }

    /// Check if an atlas texture is loaded.
    fn has_atlas(&self, atlas_id: u8) -> bool {
        self.textures.contains_key(&atlas_id)
    }

    /// Returns cache statistics for profiling: (glow_count, dye_count, char_dye_count, estimated_bytes).
    /// Estimated bytes covers only the CPU-side image data retained in caches.
    #[cfg(feature = "profiling")]
    pub fn cache_stats(&self) -> (usize, usize, usize, usize) {
        let glow_count = self.glow_cache.len();
        let dye_count = self.dye_cache.len();
        let char_dye_count = self.char_dye_cache.len();
        let outlined_count = self.outlined_cache.len();

        let dye_bytes: usize = self
            .dye_cache
            .values()
            .map(|e| (e.width * e.height * 4) as usize)
            .sum();
        let char_dye_bytes: usize = self
            .char_dye_cache
            .values()
            .map(|e| (e.width * e.height * 4) as usize)
            .sum();
        let atlas_bytes: usize = self
            .atlas_images
            .values()
            .map(|img| (img.width() * img.height() * 4) as usize)
            .sum();

        // outlined_count reported via glow_count field (repurposed as total cached sprites)
        // We sum glow + outlined for the "baked" count
        (
            glow_count + outlined_count,
            dye_count,
            char_dye_count,
            dye_bytes + char_dye_bytes + atlas_bytes,
        )
    }

    /// Draw a sprite by sheet name and index -- for sprites not registered as
    /// objects (e.g. player discovery icons) but with known sheet coordinates.
    pub fn draw_sprite_by_sheet(
        &self,
        ui: &egui::Ui,
        sheet_name: &str,
        index: i32,
        rect: Rect,
    ) -> bool {
        self.draw_sprite_by_sheet_tinted(ui, sheet_name, index, rect, Color32::WHITE)
    }

    /// Like [`Self::draw_sprite_by_sheet`] but multiplies the sprite by `tint`,
    /// so a dark tint darkens only the sprite's own pixels (alpha-masked) rather
    /// than covering the tile with a rectangle.
    pub fn draw_sprite_by_sheet_tinted(
        &self,
        ui: &egui::Ui,
        sheet_name: &str,
        index: i32,
        rect: Rect,
        tint: Color32,
    ) -> bool {
        let sprite_data = match get_asset_manager().get_sprite(sheet_name, index) {
            Some(data) => data,
            None => return false,
        };

        if sprite_data.width < 4 || sprite_data.height < 4 {
            return false;
        }

        if !self.has_atlas(sprite_data.atlas_id) {
            return false;
        }

        let texture = match self.textures.get(&sprite_data.atlas_id) {
            Some(t) => t,
            None => return false,
        };

        let (atlas_w, atlas_h) = match self.atlas_dimensions.get(&sprite_data.atlas_id) {
            Some(&dims) => dims,
            None => return false,
        };
        if Self::checked_region(
            sprite_data.x,
            sprite_data.y,
            sprite_data.width,
            sprite_data.height,
            atlas_w,
            atlas_h,
        )
        .is_none()
        {
            return false;
        }

        let uv_min = Pos2::new(
            sprite_data.x as f32 / atlas_w as f32,
            sprite_data.y as f32 / atlas_h as f32,
        );
        let uv_max = Pos2::new(
            (sprite_data.x + sprite_data.width) as f32 / atlas_w as f32,
            (sprite_data.y + sprite_data.height) as f32 / atlas_h as f32,
        );
        let uv = Rect::from_min_max(uv_min, uv_max);

        // Pixel-perfect scaling
        let dpi_scale = ui.ctx().pixels_per_point();
        let sprite_w = sprite_data.width as f32;
        let sprite_h = sprite_data.height as f32;
        let physical_rect_w = rect.width() * dpi_scale;
        let physical_rect_h = rect.height() * dpi_scale;
        let raw_scale = (physical_rect_w / sprite_w).min(physical_rect_h / sprite_h);
        let physical_scale = if raw_scale >= 1.0 {
            raw_scale.floor()
        } else {
            raw_scale
        };
        let scaled_w = (sprite_w * physical_scale) / dpi_scale;
        let scaled_h = (sprite_h * physical_scale) / dpi_scale;
        let draw_rect = Rect::from_center_size(rect.center(), egui::vec2(scaled_w, scaled_h));

        ui.painter().image(texture.id(), draw_rect, uv, tint);
        true
    }

    /// Draw a forge/tooltip category icon (a frame of the `CollectionIcon`
    /// sheet) into `rect`. Frames are a fixed 16x16 grid, row-major.
    ///
    /// Returns false (drawing nothing) if the sheet isn't loaded, its geometry
    /// is unexpected, or the index is out of range, so callers can fall back to
    /// a text-only label.
    pub fn draw_collection_icon(&self, ui: &egui::Ui, index: i32, rect: Rect) -> bool {
        const FRAME: u32 = 16;

        let Some(texture) = self.collection_icon_texture.as_ref() else {
            return false;
        };
        let (sheet_w, sheet_h) = self.collection_icon_dims;
        if sheet_w < FRAME || sheet_h < FRAME || sheet_w % FRAME != 0 || sheet_h % FRAME != 0 {
            return false;
        }
        let cols = sheet_w / FRAME;
        let rows = sheet_h / FRAME;
        if index < 0 || (index as u32) >= cols * rows {
            return false;
        }
        let col = index as u32 % cols;
        let row = index as u32 / cols;

        let uv_min = Pos2::new(
            (col * FRAME) as f32 / sheet_w as f32,
            (row * FRAME) as f32 / sheet_h as f32,
        );
        let uv_max = Pos2::new(
            (col * FRAME + FRAME) as f32 / sheet_w as f32,
            (row * FRAME + FRAME) as f32 / sheet_h as f32,
        );
        let uv = Rect::from_min_max(uv_min, uv_max);

        // Pixel-perfect scaling (mirrors draw_sprite_by_sheet).
        let dpi_scale = ui.ctx().pixels_per_point();
        let physical_rect_w = rect.width() * dpi_scale;
        let physical_rect_h = rect.height() * dpi_scale;
        let raw_scale = (physical_rect_w / FRAME as f32).min(physical_rect_h / FRAME as f32);
        let physical_scale = if raw_scale >= 1.0 {
            raw_scale.floor()
        } else {
            raw_scale
        };
        let scaled = (FRAME as f32 * physical_scale) / dpi_scale;
        let draw_rect = Rect::from_center_size(rect.center(), egui::vec2(scaled, scaled));

        ui.painter()
            .image(texture.id(), draw_rect, uv, Color32::WHITE);
        true
    }

    /// Map an embedded legacy-portal sentinel id (from
    /// `get_dungeon_portal_map().get_portal_id`) to its embedded icon, or
    /// `None` for ordinary object ids.
    fn legacy_embed_icon(item_id: i32) -> Option<EmbeddedIcon> {
        realmhound_core::assets::legacy_embed_portal_index(item_id)
            .and_then(EmbeddedIcon::legacy_portal)
    }

    /// Compute an integer-scaled, centered rect for an embedded icon so it
    /// renders at the same physical size as an equivalent atlas sprite (which
    /// integer-scales its native pixels and centers them in the cell) instead
    /// of stretching to fill the whole rect.
    fn embedded_scaled_rect(&self, ui: &egui::Ui, icon: EmbeddedIcon, rect: Rect) -> Option<Rect> {
        let size = self.embedded_icons.get(&icon)?.size();
        let (sprite_w, sprite_h) = (size[0] as f32, size[1] as f32);
        if sprite_w < 1.0 || sprite_h < 1.0 {
            return None;
        }
        let dpi_scale = ui.ctx().pixels_per_point();
        let raw_scale =
            ((rect.width() * dpi_scale) / sprite_w).min((rect.height() * dpi_scale) / sprite_h);
        let physical_scale = if raw_scale >= 1.0 {
            raw_scale.floor()
        } else {
            raw_scale
        };
        let scaled_w = (sprite_w * physical_scale) / dpi_scale;
        let scaled_h = (sprite_h * physical_scale) / dpi_scale;
        Some(Rect::from_center_size(
            rect.center(),
            egui::vec2(scaled_w, scaled_h),
        ))
    }

    /// Draw an embedded icon (from PNG assets compiled into the binary).
    pub fn draw_embedded_icon(&self, ui: &egui::Ui, icon: EmbeddedIcon, rect: Rect) -> bool {
        let Some(texture) = self.embedded_icons.get(&icon) else {
            return false;
        };

        // Draw at full rect size with nearest-neighbor filtering (already set in texture options)
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        ui.painter().image(texture.id(), rect, uv, Color32::WHITE);
        true
    }

    /// Like [`Self::draw_embedded_icon`] but multiplies the icon by `tint`
    /// (alpha-masked), so callers can dim an embedded icon (e.g. a claimed
    /// mission's objective marker) without a covering rect.
    pub fn draw_embedded_icon_tinted(
        &self,
        ui: &egui::Ui,
        icon: EmbeddedIcon,
        rect: Rect,
        tint: Color32,
    ) -> bool {
        let Some(texture) = self.embedded_icons.get(&icon) else {
            return false;
        };
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        ui.painter().image(texture.id(), rect, uv, tint);
        true
    }

    /// Draw a tier-specific dungeon-modifier icon into `rect`. Returns false if
    /// the icon hasn't been loaded yet.
    pub fn draw_modifier_icon(&self, ui: &egui::Ui, icon: ModTierIcon, rect: Rect) -> bool {
        let Some(texture) = self.modifier_icons.get(&icon) else {
            return false;
        };
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        ui.painter().image(texture.id(), rect, uv, Color32::WHITE);
        true
    }
    /// Apply tab-header display settings. Call on startup and whenever the
    /// appearance settings change so tab/sub-tab rendering stays in sync.
    pub fn set_tab_display(
        &mut self,
        mode: realmhound_core::settings::TabLabelMode,
        tooltips: bool,
    ) {
        self.tab_label_mode = mode;
        self.tab_tooltips = tooltips;
    }

    /// Render an icon+text button with selection state, for tab bars and similar
    /// UI elements. If `label_color` is `Some` it overrides the default text
    /// color. `tooltip` is the descriptive hover text for main tabs (`None` for
    /// sub-tabs). Icon/label visibility and the effective tooltip are derived
    /// from the configured [`TabLabelMode`] and tooltip toggle. Returns the
    /// button `Response` (check `.clicked()`).
    pub fn render_icon_button(
        &mut self,
        ui: &mut egui::Ui,
        icon: Option<TabIconSprite>,
        label: &str,
        is_selected: bool,
        label_color: Option<Color32>,
        tooltip: Option<&str>,
    ) -> egui::Response {
        use realmhound_core::settings::TabLabelMode;

        // Resolve the hover text before the mode remap hides the label.
        let effective_tooltip: Option<String> = if self.tab_tooltips {
            match self.tab_label_mode {
                // Icons-only hides the name, so surface it in the tooltip.
                TabLabelMode::IconsOnly => (!label.is_empty()).then(|| label.to_string()),
                // Otherwise keep the descriptive tooltip (main tabs only).
                _ => tooltip.filter(|t| !t.is_empty()).map(|t| t.to_string()),
            }
        } else {
            None
        };

        // Remap icon/label visibility per the configured display mode.
        let (icon, label): (Option<TabIconSprite>, &str) = match self.tab_label_mode {
            TabLabelMode::IconsAndText => (icon, label),
            TabLabelMode::IconsOnly => (icon, ""),
            TabLabelMode::TextOnly => (None, label),
        };

        // Calculate button size: icon + spacing + text
        let font_id = egui::TextStyle::Button.resolve(ui.style());
        let text_width = ui.fonts_mut(|f| {
            f.layout_no_wrap(label.to_string(), font_id.clone(), Color32::WHITE)
                .rect
                .width()
        });
        let has_icon = icon.is_some();
        let icon_space = if has_icon { TAB_ICON_SIZE + 4.0 } else { 0.0 };
        let button_height = (TAB_ICON_SIZE + 8.0).max(ui.spacing().interact_size.y); // fit icon with padding
                                                                                     // Icons-only tabs render as a square button with the icon centered
                                                                                     // (no trailing label space), so both margins stay equal.
        let icon_only = has_icon && label.is_empty();
        let button_width = if icon_only {
            button_height
        } else {
            icon_space + text_width + 12.0 // icon + spacing + text + padding
        };

        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(button_width, button_height),
            egui::Sense::click(),
        );

        // Draw selection background
        let visuals = ui.style().interact_selectable(&response, is_selected);
        let bg_fill = if is_selected {
            // Selected tab uses a lighter shade of the normal tab color rather
            // than the theme accent.
            lighten_color(ui.visuals().widgets.inactive.bg_fill, 28)
        } else {
            visuals.bg_fill
        };
        ui.painter().rect(
            rect,
            visuals.corner_radius,
            bg_fill,
            visuals.bg_stroke,
            egui::StrokeKind::Outside,
        );

        let mut text_x = rect.min.x + 4.0;

        // Draw icon on the left (if available), or centered when icon-only.
        if let Some(icon_sprite) = icon {
            let icon_x = if icon_only {
                rect.center().x - TAB_ICON_SIZE / 2.0
            } else {
                rect.min.x + 4.0
            };
            let icon_rect = egui::Rect::from_min_size(
                egui::pos2(icon_x, rect.center().y - TAB_ICON_SIZE / 2.0),
                egui::vec2(TAB_ICON_SIZE, TAB_ICON_SIZE),
            );

            let icon_drawn = match icon_sprite {
                TabIconSprite::ObjectId(id) => self.draw_outlined_sprite_in_rect(ui, id, icon_rect),
                TabIconSprite::ObjectIdFilled(id) => {
                    self.draw_outlined_sprite_in_rect_filled(ui, id, icon_rect)
                }
                TabIconSprite::SheetIndex(sheet, index) => {
                    self.draw_outlined_sprite_by_sheet(ui, sheet, index, icon_rect)
                }
                TabIconSprite::Embedded(embedded) => {
                    self.draw_outlined_embedded_icon(ui, embedded, icon_rect)
                }
            };

            // Fallback: draw a placeholder if icon failed
            if !icon_drawn {
                ui.painter()
                    .rect_filled(icon_rect, 2.0, Color32::from_gray(60));
            }

            text_x = icon_rect.max.x + 4.0;
        }

        // Draw label using painter (doesn't interfere with click handling)
        let text_pos = egui::pos2(
            text_x,
            rect.center().y - ui.text_style_height(&egui::TextStyle::Button) / 2.0,
        );
        let text_color = if is_selected {
            Color32::WHITE
        } else {
            label_color.unwrap_or_else(|| visuals.text_color())
        };
        ui.painter()
            .text(text_pos, egui::Align2::LEFT_TOP, label, font_id, text_color);

        if let Some(tip) = effective_tooltip {
            response.hover_tip(tip)
        } else {
            response
        }
    }

    /// Render an icon sprite in a rect (no button chrome), e.g. inline with text
    /// in collapsible section headers. Returns true if the icon was rendered.
    pub fn render_icon(&mut self, ui: &egui::Ui, icon: Option<TabIconSprite>, rect: Rect) -> bool {
        let Some(icon_sprite) = icon else {
            return false;
        };

        let icon_drawn = match icon_sprite {
            TabIconSprite::ObjectId(id) => self.draw_sprite_in_rect(ui, id, rect),
            TabIconSprite::ObjectIdFilled(id) => {
                self.draw_outlined_sprite_in_rect_filled(ui, id, rect)
            }
            TabIconSprite::SheetIndex(sheet, index) => {
                self.draw_sprite_by_sheet(ui, sheet, index, rect)
            }
            TabIconSprite::Embedded(embedded) => self.draw_embedded_icon(ui, embedded, rect),
        };

        // Fallback: draw a placeholder if icon failed
        if !icon_drawn {
            ui.painter().rect_filled(rect, 2.0, Color32::from_gray(60));
        }

        icon_drawn
    }

    /// Draw a sprite with custom rect (for integration with existing code).
    /// Uses the standard sprite lookup (not direction-aware).
    /// Automatically generates dye-composited textures for dye/cloth items.
    pub fn draw_sprite_in_rect(&mut self, ui: &egui::Ui, item_id: i32, rect: Rect) -> bool {
        self.draw_sprite_in_rect_tinted(ui, item_id, rect, Color32::WHITE)
    }

    /// Same as [`Self::draw_sprite_in_rect`] but multiplies the sprite image by
    /// `tint` (e.g. a semi-transparent white to fade the icon).
    pub fn draw_sprite_in_rect_tinted(
        &mut self,
        ui: &egui::Ui,
        item_id: i32,
        rect: Rect,
        tint: Color32,
    ) -> bool {
        // Cull off-screen sprites: skip the asset lookup + paint entirely when the
        // rect is outside the current clip (e.g. rows scrolled out of a list).
        if !ui.is_rect_visible(rect) {
            return true;
        }
        // Embedded legacy portals ship as PNGs rather than atlas sprites. Match
        // the atlas path's integer-scale-and-center sizing so they render the
        // same size as real portal sprites (not stretched to fill the rect).
        if let Some(icon) = Self::legacy_embed_icon(item_id) {
            let draw_rect = self.embedded_scaled_rect(ui, icon, rect).unwrap_or(rect);
            return self.draw_embedded_icon_tinted(ui, icon, draw_rect, tint);
        }
        // Ensure dye texture is ready (no-op for non-dye items)
        self.ensure_dye_cache(ui.ctx(), item_id);
        // Use dye-composited texture if available
        if let Some(entry) = self.dye_cache.get(&item_id) {
            let dpi_scale = ui.ctx().pixels_per_point();
            let sprite_w = entry.width as f32;
            let sprite_h = entry.height as f32;
            let physical_rect_w = rect.width() * dpi_scale;
            let physical_rect_h = rect.height() * dpi_scale;
            let raw_scale = (physical_rect_w / sprite_w).min(physical_rect_h / sprite_h);
            let physical_scale = if raw_scale >= 1.0 {
                raw_scale.floor()
            } else {
                raw_scale
            };
            let scaled_w = (sprite_w * physical_scale) / dpi_scale;
            let scaled_h = (sprite_h * physical_scale) / dpi_scale;
            let draw_rect = Rect::from_center_size(rect.center(), egui::vec2(scaled_w, scaled_h));
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            ui.painter().image(entry.texture.id(), draw_rect, uv, tint);
            return true;
        }

        // Use the standard sprite lookup (not direction-aware)
        // This works for items, bags, and mobs that don't have animated sprites
        let sprite_data = match get_asset_manager().get_object_sprite(item_id) {
            Some(data) => data,
            None => {
                return false;
            }
        };

        // Skip sprites that are too small (likely placeholders or errors)
        if sprite_data.width < 4 || sprite_data.height < 4 {
            return false;
        }

        // Check if atlas is loaded
        if !self.has_atlas(sprite_data.atlas_id) {
            //tracing::debug!("Atlas {} not loaded for id={}", sprite_data.atlas_id, item_id);
            return false;
        }

        // Trim fully-transparent padding so the visible art centers in `rect`
        // and scales to fill it (fixes padded sheets rendering small/off-center).
        let Some((sprite_x, sprite_y, sprite_pw, sprite_ph)) = self.trimmed_region(
            sprite_data.atlas_id,
            sprite_data.x,
            sprite_data.y,
            sprite_data.width,
            sprite_data.height,
        ) else {
            return false;
        };

        let texture = match self.textures.get(&sprite_data.atlas_id) {
            Some(t) => t,
            None => return false,
        };

        let (atlas_w, atlas_h) = match self.atlas_dimensions.get(&sprite_data.atlas_id) {
            Some(&dims) => dims,
            None => return false,
        };
        if Self::checked_region(
            sprite_data.x,
            sprite_data.y,
            sprite_data.width,
            sprite_data.height,
            atlas_w,
            atlas_h,
        )
        .is_none()
        {
            return false;
        }

        // Calculate UV coordinates
        let uv_min = Pos2::new(
            sprite_x as f32 / atlas_w as f32,
            sprite_y as f32 / atlas_h as f32,
        );
        let uv_max = Pos2::new(
            (sprite_x + sprite_pw) as f32 / atlas_w as f32,
            (sprite_y + sprite_ph) as f32 / atlas_h as f32,
        );

        let uv = Rect::from_min_max(uv_min, uv_max);

        // Pixel-perfect scaling accounting for DPI
        // We need physical pixels to be an integer multiple of sprite size
        let dpi_scale = ui.ctx().pixels_per_point();
        let sprite_w = sprite_pw as f32;
        let sprite_h = sprite_ph as f32;

        // Calculate the maximum physical pixel scale that fits in the rect
        let physical_rect_w = rect.width() * dpi_scale;
        let physical_rect_h = rect.height() * dpi_scale;
        let raw_scale = (physical_rect_w / sprite_w).min(physical_rect_h / sprite_h);
        // Use integer scaling when sprite fits (crisp pixels), fractional when it doesn't
        let physical_scale = if raw_scale >= 1.0 {
            raw_scale.floor()
        } else {
            raw_scale
        };

        // Convert back to logical pixels
        let scaled_w = (sprite_w * physical_scale) / dpi_scale;
        let scaled_h = (sprite_h * physical_scale) / dpi_scale;

        // Center the scaled sprite in the rect
        let center = rect.center();
        let draw_rect = Rect::from_center_size(center, egui::vec2(scaled_w, scaled_h));

        ui.painter().image(texture.id(), draw_rect, uv, tint);

        true
    }

    /// If `item_id` is a forge Blueprint, stamp the unlocked item's icon in the
    /// top-left corner of `slot_rect` with a 1px black outline in the shape of
    /// the item (matching the Quests reward look). No-op for non-blueprints, so
    /// this is safe to call for any item icon.
    pub fn draw_blueprint_unlock_overlay(&mut self, ui: &egui::Ui, item_id: i32, slot_rect: Rect) {
        let unlocked_id = match get_asset_manager().blueprint_unlocked_item(item_id) {
            Some((id, _)) => id,
            None => return,
        };
        let o = (slot_rect.width() * 0.55).max(12.0).min(slot_rect.width());
        let overlay = Rect::from_min_size(slot_rect.left_top(), egui::vec2(o, o));
        // Black silhouette outline: draw the sprite tinted solid black offset in
        // 8 directions, then the real sprite on top.
        for (dx, dy) in [
            (-1.0, 0.0),
            (1.0, 0.0),
            (0.0, -1.0),
            (0.0, 1.0),
            (-1.0, -1.0),
            (1.0, -1.0),
            (-1.0, 1.0),
            (1.0, 1.0),
        ] {
            let r = overlay.translate(egui::vec2(dx, dy));
            self.draw_sprite_in_rect_tinted(ui, unlocked_id, r, Color32::BLACK);
        }
        self.draw_sprite_in_rect(ui, unlocked_id, overlay);
    }

    ///
    /// `tier_idx`: 0 = Uncommon (1 enchant), 1 = Rare (2), 2 = Legendary (3),
    /// 3 = Divine (4+). Returns false if the overlay textures haven't been
    /// loaded yet (call `load_overlay_sprites` once at startup).
    pub fn draw_enchant_tier_in_rect(&self, ui: &egui::Ui, tier_idx: usize, rect: Rect) -> bool {
        self.draw_enchant_tier_in_rect_tinted(ui, tier_idx, rect, Color32::WHITE)
    }

    /// Same as [`Self::draw_enchant_tier_in_rect`], but multiplies the gem image
    /// by `tint` (e.g. a dark grey to darken it alongside a darkened sprite for
    /// not-yet-unlocked rarities).
    pub fn draw_enchant_tier_in_rect_tinted(
        &self,
        ui: &egui::Ui,
        tier_idx: usize,
        rect: Rect,
        tint: Color32,
    ) -> bool {
        let tex = match self.enchant_textures.get(tier_idx).and_then(|t| t.as_ref()) {
            Some(t) => t,
            None => return false,
        };
        // Enchant sprites are 16x16; use pixel-perfect integer scaling.
        let dpi_scale = ui.ctx().pixels_per_point();
        let sprite_native = 16.0_f32;
        let physical_rect = rect.width().min(rect.height()) * dpi_scale;
        let physical_scale = (physical_rect / sprite_native).floor().max(1.0);
        let size = (sprite_native * physical_scale) / dpi_scale;
        let draw_rect = Rect::from_center_size(rect.center(), egui::vec2(size, size));
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        ui.painter().image(tex.id(), draw_rect, uv, tint);
        true
    }

    /// Draw the enchant-tier gem scaled to fill `rect` exactly (fractional, not
    /// integer-snapped). Unlike [`Self::draw_enchant_tier_in_rect`], this honors
    /// rects smaller than the 16px native gem, so it can be used for small corner
    /// badges over item icons.
    pub fn draw_enchant_tier_in_rect_scaled(
        &self,
        ui: &egui::Ui,
        tier_idx: usize,
        rect: Rect,
    ) -> bool {
        let tex = match self
            .enchant_textures_smooth
            .get(tier_idx)
            .and_then(|t| t.as_ref())
        {
            Some(t) => t,
            None => return false,
        };
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        ui.painter().image(tex.id(), rect, uv, Color32::WHITE);
        true
    }
    /// Uses the same technique as enchanted items: draws 8 offset black copies then
    /// the real sprite on top.  If the item has a dye-composited texture, that is
    /// drawn instead of the raw atlas sprite.
    pub fn draw_outlined_sprite_in_rect(
        &mut self,
        ui: &egui::Ui,
        item_id: i32,
        rect: Rect,
    ) -> bool {
        if !ui.is_rect_visible(rect) {
            return true;
        }

        if let Some(icon) = Self::legacy_embed_icon(item_id) {
            return self.draw_outlined_embedded_icon(ui, icon, rect);
        }

        self.ensure_dye_cache(ui.ctx(), item_id);

        // Compute integer scale for this sprite in this rect
        let scale = self.compute_sprite_scale(item_id, ui, rect);
        if scale == 0 {
            return false;
        }

        if self.ensure_outlined_cache(ui.ctx(), item_id, scale) {
            let offset = self.content_offset_display(item_id, ui, scale);
            let texture = self.outlined_cache.get(&(item_id, scale)).unwrap();
            let draw_rect = Self::centered_rect_for_texture(ui, texture, rect, offset);
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            ui.painter()
                .image(texture.id(), draw_rect, uv, Color32::WHITE);
            count_draw_call();
            count_outlined_sprite();
            return true;
        }

        false
    }

    /// Draws the shiny sparkle overlay in the top-left corner of `rect`,
    /// sized to 1/4 of the rect (matching the "All Items" item-slot rendering).
    /// Pass the full slot rect (not the inset art rect) so the sparkle has the
    /// same top/left margin as the loot tiles and isn't clipped. `tint` allows
    /// darkening the sparkle for unobtained collection items.
    pub fn draw_shiny_overlay_in_rect(&self, ui: &egui::Ui, rect: Rect, tint: Color32) {
        let Some(shiny_tex) = &self.shiny_texture else {
            return;
        };
        if !ui.is_rect_visible(rect) {
            return;
        }
        let dpi_scale = ui.ctx().pixels_per_point();
        let target_physical = (rect.width() / 4.0 * dpi_scale).round();
        let overlay_size = target_physical / dpi_scale;
        let shiny_rect =
            Rect::from_min_size(rect.left_top(), Vec2::new(overlay_size, overlay_size));
        ui.painter().image(
            shiny_tex.id(),
            shiny_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            tint,
        );
    }

    /// Like [`Self::draw_outlined_sprite_in_rect`] but scales by the sprite's
    /// native frame size rather than its trimmed bounds, so padded item art
    /// renders at a consistent proportion (matching the enchant-glow path)
    /// instead of overflowing the cell. Used for gear item icons.
    pub fn draw_outlined_sprite_in_rect_native(
        &mut self,
        ui: &egui::Ui,
        item_id: i32,
        rect: Rect,
    ) -> bool {
        if !ui.is_rect_visible(rect) {
            return true;
        }

        self.ensure_dye_cache(ui.ctx(), item_id);

        let scale = self.compute_sprite_scale_ex(item_id, ui, rect, false);
        if scale == 0 {
            return false;
        }

        if self.ensure_outlined_cache(ui.ctx(), item_id, scale) {
            let offset = self.content_offset_display(item_id, ui, scale);
            let texture = self.outlined_cache.get(&(item_id, scale)).unwrap();
            let draw_rect = Self::centered_rect_for_texture(ui, texture, rect, offset);
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            ui.painter()
                .image(texture.id(), draw_rect, uv, Color32::WHITE);
            count_draw_call();
            count_outlined_sprite();
            return true;
        }

        false
    }

    /// Like [`Self::draw_outlined_sprite_in_rect_native`] but applies `tint`
    /// (used to render unobtained collection items darkened, while keeping the
    /// same native-frame scaling/padding as obtained items and loot tiles).
    pub fn draw_outlined_sprite_in_rect_native_tinted(
        &mut self,
        ui: &egui::Ui,
        item_id: i32,
        rect: Rect,
        tint: Color32,
    ) -> bool {
        if !ui.is_rect_visible(rect) {
            return true;
        }

        self.ensure_dye_cache(ui.ctx(), item_id);

        let scale = self.compute_sprite_scale_ex(item_id, ui, rect, false);
        if scale == 0 {
            return false;
        }

        if self.ensure_outlined_cache(ui.ctx(), item_id, scale) {
            let offset = self.content_offset_display(item_id, ui, scale);
            let texture = self.outlined_cache.get(&(item_id, scale)).unwrap();
            let draw_rect = Self::centered_rect_for_texture(ui, texture, rect, offset);
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            ui.painter().image(texture.id(), draw_rect, uv, tint);
            count_draw_call();
            count_outlined_sprite();
            return true;
        }

        false
    }

    /// Draw a sprite's baked contour-outlined texture scaled to *fill* `rect`
    /// (aspect-preserving, fractional). Unlike [`Self::draw_outlined_sprite_in_rect`]
    /// which uses crisp integer scaling and can leave physically-large sprites
    /// (e.g. 16x16 "big" portal art) small inside a tab-icon box, this upscales
    /// to match the visual size of sprites whose native art already fills the box.
    pub fn draw_outlined_sprite_in_rect_filled(
        &mut self,
        ui: &egui::Ui,
        item_id: i32,
        rect: Rect,
    ) -> bool {
        if !ui.is_rect_visible(rect) {
            return true;
        }

        if let Some(icon) = Self::legacy_embed_icon(item_id) {
            return self.draw_outlined_embedded_icon(ui, icon, rect);
        }

        self.ensure_dye_cache(ui.ctx(), item_id);

        let scale = self.compute_sprite_scale(item_id, ui, rect);
        if scale == 0 {
            return false;
        }

        if self.ensure_outlined_cache(ui.ctx(), item_id, scale) {
            let texture = self.outlined_cache.get(&(item_id, scale)).unwrap();
            let dpi_scale = ui.ctx().pixels_per_point();
            let tex_w = texture.size()[0] as f32 / dpi_scale;
            let tex_h = texture.size()[1] as f32 / dpi_scale;
            if tex_w <= 0.0 || tex_h <= 0.0 {
                return false;
            }
            let factor = (rect.width() / tex_w).min(rect.height() / tex_h);
            let draw_rect =
                Rect::from_center_size(rect.center(), egui::vec2(tex_w * factor, tex_h * factor));
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            ui.painter()
                .image(texture.id(), draw_rect, uv, Color32::WHITE);
            count_draw_call();
            count_outlined_sprite();
            return true;
        }

        false
    }

    /// Draw an object sprite with a 1px black outline, extending the source
    /// region below the authored `position` rect so contiguous art the rect
    /// crops off is included (e.g. exaltation banners recorded as 24x16 while
    /// the full banner with pole/tassels spans ~24x31 in the atlas). The full
    /// opaque region within a `cell_h`-tall box anchored at the sprite origin is
    /// trimmed to its opaque bounds, baked with an outline, and fit into `rect`.
    pub fn draw_object_full_outlined_in_rect(
        &mut self,
        ui: &egui::Ui,
        item_id: i32,
        rect: Rect,
        cell_h: i32,
    ) -> bool {
        if !ui.is_rect_visible(rect) {
            return true;
        }

        let sprite = match get_asset_manager().get_object_sprite(item_id) {
            Some(s) => s,
            None => return false,
        };
        let (aw, ah) = match self.atlas_dimensions.get(&sprite.atlas_id) {
            Some(&d) => (d.0 as i32, d.1 as i32),
            None => return false,
        };

        // Extend the source box downward to `cell_h`, then trim to opaque bounds
        // so the pole/tassels below the authored rect are captured.
        if sprite.x < 0 || sprite.y < 0 || sprite.x >= aw || sprite.y >= ah {
            return false;
        }
        let box_w = sprite.width.max(1).min(aw - sprite.x);
        let box_h = cell_h.max(sprite.height).min(ah - sprite.y);
        let Some((tx, ty, tw, th)) =
            self.trimmed_region(sprite.atlas_id, sprite.x, sprite.y, box_w, box_h)
        else {
            return false;
        };
        if tw < 4 || th < 4 {
            return false;
        }

        // Integer scale that fits the trimmed region into `rect`.
        let dpi = ui.ctx().pixels_per_point();
        let raw = ((rect.width() * dpi) / tw as f32).min((rect.height() * dpi) / th as f32);
        let scale = if raw >= 1.0 { raw.floor() as u32 } else { 1 }.max(1);

        if self.ensure_outlined_full_cache(
            ui.ctx(),
            item_id,
            scale,
            sprite.atlas_id,
            tx,
            ty,
            tw,
            th,
        ) {
            let texture = self.outlined_full_cache.get(&(item_id, scale)).unwrap();
            let draw_rect = Self::centered_rect_for_texture(ui, texture, rect, egui::Vec2::ZERO);
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            ui.painter()
                .image(texture.id(), draw_rect, uv, Color32::WHITE);
            count_draw_call();
            count_outlined_sprite();
            return true;
        }

        false
    }

    #[allow(clippy::too_many_arguments)]
    fn ensure_outlined_full_cache(
        &mut self,
        ctx: &egui::Context,
        item_id: i32,
        scale: u32,
        atlas_id: u8,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    ) -> bool {
        let key = (item_id, scale);
        if self.outlined_full_cache.contains_key(&key) {
            return true;
        }
        let atlas = match self.atlas_images.get(&atlas_id) {
            Some(a) => a,
            None => return false,
        };
        let src = match Self::crop_atlas_region(atlas_id, atlas, x, y, w, h) {
            Some(image) => image,
            None => return false,
        };
        let scaled = image::imageops::resize(
            &src,
            w as u32 * scale,
            h as u32 * scale,
            image::imageops::FilterType::Nearest,
        );
        if scaled.width() == 0 || scaled.height() == 0 {
            return false;
        }
        let baked = Self::bake_outline(&scaled);
        let color_image = ColorImage::from_rgba_unmultiplied(
            [baked.width() as usize, baked.height() as usize],
            baked.as_raw(),
        );
        let texture = ctx.load_texture(
            format!("outlined_full_{}_{}x", item_id, scale),
            color_image,
            TextureOptions::NEAREST,
        );
        self.outlined_full_cache.insert(key, texture);
        true
    }

    /// Compute the integer scale for a sprite displayed in `rect`.
    /// Returns 0 if the sprite can't be resolved.
    fn compute_sprite_scale(&mut self, item_id: i32, ui: &egui::Ui, rect: Rect) -> u32 {
        self.compute_sprite_scale_ex(item_id, ui, rect, true)
    }

    /// Integer scale for a sprite in `rect`. When `fill_trimmed` is true the
    /// scale is derived from the sprite's trimmed (opaque) bounds so padded art
    /// fills the cell (used by header/portal icons). When false it uses the
    /// native frame size, matching the enchant-glow path so items render at a
    /// consistent proportion instead of overflowing their cell.
    fn compute_sprite_scale_ex(
        &mut self,
        item_id: i32,
        ui: &egui::Ui,
        rect: Rect,
        fill_trimmed: bool,
    ) -> u32 {
        // Always scale relative to the NATIVE base sprite size (not the dye
        // composite, which may be pre-upscaled). This keeps dye/cloth items on
        // the same integer-scaling path as normal sprites so their baked outline
        // stays a crisp 1px. Padded sheets scale by their trimmed (opaque) size
        // so the visible art fills the cell instead of rendering small.
        let sprite_data = match get_asset_manager().get_object_sprite(item_id) {
            Some(data) => data,
            None => return 0,
        };
        if sprite_data.width < 4 || sprite_data.height < 4 {
            return 0;
        }
        let (sprite_w, sprite_h) = if fill_trimmed {
            let Some((_, _, tw, th)) = self.trimmed_region(
                sprite_data.atlas_id,
                sprite_data.x,
                sprite_data.y,
                sprite_data.width,
                sprite_data.height,
            ) else {
                return 0;
            };
            if dense_trim_for_scale(item_id) {
                // Ignore thin protrusions (sparse edge rows/cols) when picking the
                // integer scale, so the main body scales up instead of a few stray
                // pixels capping it. Plagued Nest's portal has two 2px "feet" at
                // the bottom corners that otherwise widen its opaque box and drop
                // it a scale step below the visually identical The Nest portal.
                let (dw, dh) = self.dense_trim_wh(
                    sprite_data.atlas_id,
                    sprite_data.x,
                    sprite_data.y,
                    sprite_data.width,
                    sprite_data.height,
                    1,
                );
                (dw.max(1) as f32, dh.max(1) as f32)
            } else {
                (tw as f32, th as f32)
            }
        } else {
            (sprite_data.width as f32, sprite_data.height as f32)
        };
        let dpi_scale = ui.ctx().pixels_per_point();
        let physical_rect_w = rect.width() * dpi_scale;
        let physical_rect_h = rect.height() * dpi_scale;
        let raw_scale = (physical_rect_w / sprite_w).min(physical_rect_h / sprite_h);
        let scale = if raw_scale >= 1.0 {
            raw_scale.floor() as u32
        } else {
            1
        };
        scale.max(1)
    }

    /// Center a baked texture in `rect` at 1:1 pixel mapping (the texture is
    /// already at the correct display size, just needs centering and DPI conversion).
    /// `offset` shifts the draw position (display points) so item content, rather
    /// than the raw sprite frame, centers in the cell.
    fn centered_rect_for_texture(
        ui: &egui::Ui,
        texture: &TextureHandle,
        rect: Rect,
        offset: egui::Vec2,
    ) -> Rect {
        let dpi_scale = ui.ctx().pixels_per_point();
        let tex_w = texture.size()[0] as f32 / dpi_scale;
        let tex_h = texture.size()[1] as f32 / dpi_scale;
        let draw = Rect::from_center_size(rect.center() + offset, egui::vec2(tex_w, tex_h));
        // Snap the top-left to the physical pixel grid. The texture is baked at
        // an integer scale (1 texel = 1 physical pixel), so an unaligned origin
        // makes NEAREST sampling drop/duplicate edge rows, producing an uneven
        // outline. Aligning to whole physical pixels keeps the border a crisp 1px.
        let snapped_min = Pos2::new(
            (draw.min.x * dpi_scale).round() / dpi_scale,
            (draw.min.y * dpi_scale).round() / dpi_scale,
        );
        Rect::from_min_size(snapped_min, draw.size())
    }

    fn checked_region(
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        atlas_width: u32,
        atlas_height: u32,
    ) -> Option<(u32, u32, u32, u32)> {
        let x = u32::try_from(x).ok()?;
        let y = u32::try_from(y).ok()?;
        let width = u32::try_from(width).ok().filter(|&value| value > 0)?;
        let height = u32::try_from(height).ok().filter(|&value| value > 0)?;
        if x.checked_add(width)? > atlas_width || y.checked_add(height)? > atlas_height {
            return None;
        }
        Some((x, y, width, height))
    }

    fn crop_atlas_region(
        atlas_id: u8,
        atlas: &DynamicImage,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Option<RgbaImage> {
        let Some((x, y, width, height)) =
            Self::checked_region(x, y, width, height, atlas.width(), atlas.height())
        else {
            tracing::debug!(
                atlas_id,
                x,
                y,
                width,
                height,
                atlas_width = atlas.width(),
                atlas_height = atlas.height(),
                "Skipping invalid sprite atlas region"
            );
            return None;
        };
        Some(atlas.crop_imm(x, y, width, height).to_rgba8())
    }

    /// Opaque bounding box `(x, y, w, h)` of a valid sprite region within the
    /// atlas, trimmed to its non-transparent pixels.
    fn trimmed_region(
        &mut self,
        atlas_id: u8,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Option<(i32, i32, i32, i32)> {
        let key = (atlas_id, x, y, width, height);
        if let Some(&r) = self.trim_cache.get(&key) {
            return Some(r);
        }
        let full = (x, y, width, height);
        let atlas = match self.atlas_images.get(&atlas_id) {
            Some(a) => a,
            None => return None,
        };
        let Some((_, _, checked_width, checked_height)) =
            Self::checked_region(x, y, width, height, atlas.width(), atlas.height())
        else {
            tracing::debug!(
                atlas_id,
                x,
                y,
                width,
                height,
                atlas_width = atlas.width(),
                atlas_height = atlas.height(),
                "Skipping invalid sprite atlas region"
            );
            return None;
        };
        let (w, h) = (checked_width as i32, checked_height as i32);
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (w, h, -1i32, -1i32);
        for dy in 0..h {
            for dx in 0..w {
                let px = atlas.get_pixel((x + dx) as u32, (y + dy) as u32);
                if px[3] > 0 {
                    min_x = min_x.min(dx);
                    min_y = min_y.min(dy);
                    max_x = max_x.max(dx);
                    max_y = max_y.max(dy);
                }
            }
        }
        let trimmed = if max_x < 0 {
            full
        } else {
            (x + min_x, y + min_y, max_x - min_x + 1, max_y - min_y + 1)
        };
        self.trim_cache.insert(key, trimmed);
        Some(trimmed)
    }

    /// Opaque bounding box `(width, height)` ignoring thin protrusions: edge
    /// rows/columns whose opaque-pixel count is `<= min_count` are treated as
    /// non-content and skipped inward. Used to pick a display scale from a
    /// sprite's main body rather than a few stray pixels (e.g. the Plagued Nest
    /// portal's corner "feet"). Not cached -- called only for a tiny allowlist.
    fn dense_trim_wh(
        &self,
        atlas_id: u8,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        min_count: u32,
    ) -> (i32, i32) {
        let atlas = match self.atlas_images.get(&atlas_id) {
            Some(a) => a,
            None => return (width, height),
        };
        if Self::checked_region(x, y, width, height, atlas.width(), atlas.height()).is_none() {
            return (width, height);
        }
        let (w, h) = (width as usize, height as usize);
        let mut cols = vec![0u32; w];
        let mut rows = vec![0u32; h];
        for dy in 0..height {
            for dx in 0..width {
                if atlas.get_pixel((x + dx) as u32, (y + dy) as u32)[3] > 0 {
                    cols[dx as usize] += 1;
                    rows[dy as usize] += 1;
                }
            }
        }
        let span = |v: &[u32]| -> Option<(usize, usize)> {
            let first = v.iter().position(|&c| c > min_count)?;
            let last = v.iter().rposition(|&c| c > min_count)?;
            Some((first, last))
        };
        match (span(&cols), span(&rows)) {
            (Some((cl, cr)), Some((rt, rb))) => ((cr - cl + 1) as i32, (rb - rt + 1) as i32),
            _ => (width, height),
        }
    }

    /// True when the sprite's native (trimmed) art is larger than `rect` in
    /// either dimension, so the integer-scaling native draw path -- which never
    /// scales below 1x -- would overflow the cell. Callers can route such
    /// sprites to the fractional fit path instead. Large boss sheets (e.g. the
    /// 32x32 encounter bosses) trip this in the 16px objective/pill tiles.
    pub fn sprite_exceeds_cell(&mut self, item_id: i32, ui: &egui::Ui, rect: Rect) -> bool {
        let sd = match get_asset_manager().get_object_sprite(item_id) {
            Some(s) => s,
            None => return false,
        };
        let Some((_, _, tw, th)) =
            self.trimmed_region(sd.atlas_id, sd.x, sd.y, sd.width, sd.height)
        else {
            return false;
        };
        let dpi = ui.ctx().pixels_per_point();
        (tw as f32) > rect.width() * dpi || (th as f32) > rect.height() * dpi
    }

    /// Logical size a sprite renders at for a `target` height, matching the
    /// integer pixel-scaling used by [`Self::draw_sprite_in_rect`] and
    /// accounting for transparent-padding trimming. Allocating exactly this
    /// size makes the sprite fill its rect with no centering padding, so tight
    /// icons align with adjacent text.
    pub fn rendered_sprite_size(&mut self, item_id: i32, target: f32, ppp: f32) -> egui::Vec2 {
        let sd = match get_asset_manager().get_object_sprite(item_id) {
            Some(s) => s,
            None => return egui::vec2(target, target),
        };
        let Some((_, _, tw, th)) =
            self.trimmed_region(sd.atlas_id, sd.x, sd.y, sd.width, sd.height)
        else {
            return egui::vec2(target, target);
        };
        let (w, h) = (tw.max(1) as f32, th.max(1) as f32);
        let raw = (target * ppp) / w.max(h);
        let scale = if raw >= 1.0 {
            raw.floor().max(1.0)
        } else {
            raw
        };
        egui::vec2(w * scale / ppp, h * scale / ppp)
    }

    /// Content-centering offset for an item, in native sprite pixels
    /// (`frame_center - content_center`). Cached per item. Returns zero for
    /// items whose visible pixels already fill/centre the frame.
    fn content_center_offset(&mut self, item_id: i32) -> egui::Vec2 {
        if let Some(off) = self.content_offsets.get(&item_id) {
            return *off;
        }
        // Only cache once resolvable (atlas loaded); otherwise retry next frame.
        match self.compute_content_offset(item_id) {
            Some(off) => {
                self.content_offsets.insert(item_id, off);
                off
            }
            None => egui::Vec2::ZERO,
        }
    }

    fn compute_content_offset(&self, item_id: i32) -> Option<egui::Vec2> {
        let sprite_data = get_asset_manager().get_object_sprite(item_id)?;
        let (w, h) = (sprite_data.width, sprite_data.height);
        if w < 4 || h < 4 {
            return None;
        }
        let atlas = self.atlas_images.get(&sprite_data.atlas_id)?;
        Self::checked_region(
            sprite_data.x,
            sprite_data.y,
            w,
            h,
            atlas.width(),
            atlas.height(),
        )?;
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (w, h, -1i32, -1i32);
        for dy in 0..h {
            for dx in 0..w {
                let px = atlas.get_pixel((sprite_data.x + dx) as u32, (sprite_data.y + dy) as u32);
                if px[3] > 0 {
                    min_x = min_x.min(dx);
                    min_y = min_y.min(dy);
                    max_x = max_x.max(dx);
                    max_y = max_y.max(dy);
                }
            }
        }
        if max_x < 0 {
            return None;
        }
        let content_cx = (min_x + max_x + 1) as f32 / 2.0;
        let content_cy = (min_y + max_y + 1) as f32 / 2.0;
        Some(egui::vec2(
            w as f32 / 2.0 - content_cx,
            h as f32 / 2.0 - content_cy,
        ))
    }

    /// Content-centering offset converted to display points for a given integer
    /// `scale`, rounded to whole physical pixels to keep the sprite crisp.
    fn content_offset_display(&mut self, item_id: i32, ui: &egui::Ui, scale: u32) -> egui::Vec2 {
        let off = self.content_center_offset(item_id);
        if off == egui::Vec2::ZERO {
            return egui::Vec2::ZERO;
        }
        let dpi_scale = ui.ctx().pixels_per_point();
        egui::vec2(
            (off.x * scale as f32).round() / dpi_scale,
            (off.y * scale as f32).round() / dpi_scale,
        )
    }

    /// Draw an outlined sprite with the same pixel-perfect integer scaling as
    /// [`draw_outlined_sprite_in_rect`], but multiplying the top image by `tint`.
    /// Used for the Live Feed dungeon portal icon so expired entries can dim the
    /// portal (grey tint) without painting a box overlay.
    pub fn draw_outlined_sprite_in_rect_tinted(
        &mut self,
        ui: &egui::Ui,
        item_id: i32,
        rect: Rect,
        tint: Color32,
    ) -> bool {
        if !ui.is_rect_visible(rect) {
            return true;
        }

        if let Some(icon) = Self::legacy_embed_icon(item_id) {
            return self.draw_outlined_embedded_icon_tinted(ui, icon, rect, tint);
        }

        self.ensure_dye_cache(ui.ctx(), item_id);

        let scale = self.compute_sprite_scale(item_id, ui, rect);
        if scale == 0 {
            return false;
        }

        if self.ensure_outlined_cache(ui.ctx(), item_id, scale) {
            let offset = self.content_offset_display(item_id, ui, scale);
            let texture = self.outlined_cache.get(&(item_id, scale)).unwrap();
            let draw_rect = Self::centered_rect_for_texture(ui, texture, rect, offset);
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            ui.painter().image(texture.id(), draw_rect, uv, tint);
            count_draw_call();
            count_outlined_sprite();
            return true;
        }

        false
    }

    /// Draw a full-frame sprite fit to `rect` with a crisp 1px outline, used for
    /// portal icons whose art fills their 16px frame (e.g. Kogbold Steamworks).
    /// Integer scaling leaves these a step smaller than dome-style portals (The
    /// Nest, Plagued Nest) in the same cell, so they look undersized next to
    /// them. The native art is smoothly (Triangle) resized to a fractional
    /// target and the outline is baked at final resolution, so the outline stays
    /// an even 1px instead of the uneven doubling that fractional NEAREST
    /// upscaling of a pre-baked 1x sprite produces.
    pub fn draw_outlined_sprite_in_rect_fit_tinted(
        &mut self,
        ui: &egui::Ui,
        item_id: i32,
        rect: Rect,
        tint: Color32,
    ) -> bool {
        if !ui.is_rect_visible(rect) {
            return true;
        }

        if let Some(icon) = Self::legacy_embed_icon(item_id) {
            return self.draw_outlined_embedded_icon_tinted(ui, icon, rect, tint);
        }

        self.ensure_dye_cache(ui.ctx(), item_id);

        let native = match self.extract_sprite_image(item_id) {
            Some(img) => img,
            None => return false,
        };
        let (nw, nh) = (native.width() as f32, native.height() as f32);
        if nw < 1.0 || nh < 1.0 {
            return false;
        }

        // Fit the content into the cell leaving 1px each side for the outline
        // border baked below. Work in physical pixels for a crisp result.
        let dpi = ui.ctx().pixels_per_point();
        let avail_w = (rect.width() * dpi - 2.0).max(1.0);
        let avail_h = (rect.height() * dpi - 2.0).max(1.0);
        let factor = (avail_w / nw).min(avail_h / nh);
        let tw = (nw * factor).round().max(1.0) as u32;
        let th = (nh * factor).round().max(1.0) as u32;

        let key = (item_id, tw, th);
        if !self.outlined_fit_cache.contains_key(&key) {
            // Skip if dye data is expected but not yet composited (avoid baking
            // an undyed sprite permanently), matching ensure_outlined_cache.
            if !self.dye_cache.contains_key(&item_id)
                && get_asset_manager().get_item_dye_info(item_id).is_some()
            {
                return false;
            }
            let resized =
                image::imageops::resize(&native, tw, th, image::imageops::FilterType::Triangle);
            let baked = Self::bake_outline(&resized);
            let color_image = ColorImage::from_rgba_unmultiplied(
                [baked.width() as usize, baked.height() as usize],
                baked.as_raw(),
            );
            let texture = ui.ctx().load_texture(
                format!("outlined_fit_{}_{}x{}", item_id, tw, th),
                color_image,
                TextureOptions::NEAREST,
            );
            self.outlined_fit_cache.insert(key, texture);
        }

        let texture = self.outlined_fit_cache.get(&key).unwrap();
        let draw_rect = Self::centered_rect_for_texture(ui, texture, rect, egui::Vec2::ZERO);
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        ui.painter().image(texture.id(), draw_rect, uv, tint);
        count_draw_call();
        count_outlined_sprite();
        true
    }

    /// Integer display scale for a native `sprite_w`x`sprite_h` sprite shown in
    /// `rect`, using the same pixel-perfect rule as [`Self::compute_sprite_scale`].
    fn integer_scale_for(sprite_w: f32, sprite_h: f32, ui: &egui::Ui, rect: Rect) -> u32 {
        if sprite_w < 1.0 || sprite_h < 1.0 {
            return 0;
        }
        let dpi_scale = ui.ctx().pixels_per_point();
        let physical_rect_w = rect.width() * dpi_scale;
        let physical_rect_h = rect.height() * dpi_scale;
        let raw_scale = (physical_rect_w / sprite_w).min(physical_rect_h / sprite_h);
        if raw_scale >= 1.0 {
            // Round (rather than floor) so icons with larger native dimensions
            // (e.g. Party 13x12) still scale up to visually fill the tab cell
            // instead of collapsing to 1x.
            (raw_scale.round() as u32).max(1)
        } else {
            1
        }
    }

    /// Draw an embedded icon with a baked 1px black outline (tab-header style).
    pub fn draw_outlined_embedded_icon(
        &mut self,
        ui: &egui::Ui,
        icon: EmbeddedIcon,
        rect: Rect,
    ) -> bool {
        self.draw_outlined_embedded_icon_tinted(ui, icon, rect, Color32::WHITE)
    }

    /// Like [`Self::draw_outlined_embedded_icon`] but multiplies the icon by
    /// `tint`, so callers (e.g. dimmed mission objectives) can grey it out.
    pub fn draw_outlined_embedded_icon_tinted(
        &mut self,
        ui: &egui::Ui,
        icon: EmbeddedIcon,
        rect: Rect,
        tint: Color32,
    ) -> bool {
        let (nw, nh) = match self.embedded_icons.get(&icon) {
            Some(t) => {
                let s = t.size();
                (s[0] as f32, s[1] as f32)
            }
            None => return false,
        };
        // Match the atlas outlined path (`compute_sprite_scale_ex`): floor the
        // raw scale rather than rounding, so legacy portal PNGs render the same
        // size as modern portal sprites in the same cell instead of a step larger.
        let scale = {
            let dpi_scale = ui.ctx().pixels_per_point();
            let raw = ((rect.width() * dpi_scale) / nw).min((rect.height() * dpi_scale) / nh);
            if raw >= 1.0 {
                raw.floor() as u32
            } else {
                0
            }
        };
        if scale == 0 {
            return false;
        }
        if self.ensure_outlined_embedded_cache(ui.ctx(), icon, scale) {
            let texture = self.outlined_embedded_cache.get(&(icon, scale)).unwrap();
            let draw_rect = Self::centered_rect_for_texture(ui, texture, rect, egui::Vec2::ZERO);
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            ui.painter().image(texture.id(), draw_rect, uv, tint);
            count_draw_call();
            count_outlined_sprite();
            return true;
        }
        false
    }

    fn ensure_outlined_embedded_cache(
        &mut self,
        ctx: &egui::Context,
        icon: EmbeddedIcon,
        scale: u32,
    ) -> bool {
        let key = (icon, scale);
        if self.outlined_embedded_cache.contains_key(&key) {
            return true;
        }
        let Ok(image) = image::load_from_memory(icon.bytes()) else {
            return false;
        };
        let src = image.to_rgba8();
        if src.width() == 0 || src.height() == 0 {
            return false;
        }
        let scaled = image::imageops::resize(
            &src,
            src.width() * scale,
            src.height() * scale,
            image::imageops::FilterType::Nearest,
        );
        let out = Self::bake_outline(&scaled);
        let color_image = ColorImage::from_rgba_unmultiplied(
            [out.width() as usize, out.height() as usize],
            out.as_raw(),
        );
        let texture = ctx.load_texture(
            format!("outlined_embedded_{}_{}x", icon.texture_name(), scale),
            color_image,
            TextureOptions::NEAREST,
        );
        self.outlined_embedded_cache.insert(key, texture);
        true
    }

    /// Draw a sheet+index sprite with a baked 1px black outline (tab-header style).
    pub fn draw_outlined_sprite_by_sheet(
        &mut self,
        ui: &egui::Ui,
        sheet_name: &'static str,
        index: i32,
        rect: Rect,
    ) -> bool {
        let sprite_data = match get_asset_manager().get_sprite(sheet_name, index) {
            Some(data) => data,
            None => return false,
        };
        if sprite_data.width < 4 || sprite_data.height < 4 {
            return false;
        }
        let scale = Self::integer_scale_for(
            sprite_data.width as f32,
            sprite_data.height as f32,
            ui,
            rect,
        );
        if scale == 0 {
            return false;
        }
        if self.ensure_outlined_sheet_cache(ui.ctx(), sheet_name, index, scale) {
            let texture = self
                .outlined_sheet_cache
                .get(&(sheet_name, index, scale))
                .unwrap();
            let draw_rect = Self::centered_rect_for_texture(ui, texture, rect, egui::Vec2::ZERO);
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            ui.painter()
                .image(texture.id(), draw_rect, uv, Color32::WHITE);
            count_draw_call();
            count_outlined_sprite();
            return true;
        }
        false
    }

    fn ensure_outlined_sheet_cache(
        &mut self,
        ctx: &egui::Context,
        sheet_name: &'static str,
        index: i32,
        scale: u32,
    ) -> bool {
        let key = (sheet_name, index, scale);
        if self.outlined_sheet_cache.contains_key(&key) {
            return true;
        }
        let sprite_data = match get_asset_manager().get_sprite(sheet_name, index) {
            Some(data) => data,
            None => return false,
        };
        if sprite_data.width < 4 || sprite_data.height < 4 {
            return false;
        }
        let atlas = match self.atlas_images.get(&sprite_data.atlas_id) {
            Some(a) => a,
            None => return false,
        };
        let cropped = match Self::crop_atlas_region(
            sprite_data.atlas_id,
            atlas,
            sprite_data.x,
            sprite_data.y,
            sprite_data.width,
            sprite_data.height,
        ) {
            Some(image) => image,
            None => return false,
        };
        let scaled = image::imageops::resize(
            &cropped,
            cropped.width() * scale,
            cropped.height() * scale,
            image::imageops::FilterType::Nearest,
        );
        let out = Self::bake_outline(&scaled);
        let color_image = ColorImage::from_rgba_unmultiplied(
            [out.width() as usize, out.height() as usize],
            out.as_raw(),
        );
        let texture = ctx.load_texture(
            format!("outlined_sheet_{}_{}_{}x", sheet_name, index, scale),
            color_image,
            TextureOptions::NEAREST,
        );
        self.outlined_sheet_cache.insert(key, texture);
        true
    }

    /// Extract stack count from item name suffix (e.g., "Shard of the Advisor x32" -> ("Shard of the Advisor", 32)).
    /// Returns (base_name, stack_count) where stack_count is 0 if no suffix found.
    ///
    /// This is useful for items where RotMG uses different item IDs for each stack size,
    /// encoding the count in the item name itself.
    pub fn extract_stack_from_name(name: &str) -> (String, u8) {
        // Look for " x<number>" at the end of the name
        if let Some(idx) = name.rfind(" x") {
            let suffix = &name[idx + 2..];
            if let Ok(count) = suffix.parse::<u8>() {
                return (name[..idx].to_string(), count);
            }
        }
        (name.to_string(), 0)
    }

    /// Render a single item with consistent styling across all panels: dark
    /// background, sprite (falling back to the item ID), enchant dots, a shiny
    /// star, and a tooltip. `size` is the slot size in pixels (e.g. 28.0 for
    /// loot, 40.0 for vault); `stack_count` of 0 or 1 hides the count.
    /// Get the glow color for a given enchant count.
    /// Based on RotMG enchantment rarity colors:
    /// - 1 enchant = Green (Uncommon)
    /// - 2 enchants = Blue (Rare)  
    /// - 3 enchants = Purple (Legendary)
    /// - 4 enchants = Gold (Divine)
    pub fn enchant_glow_color(enchant_count: usize) -> Color32 {
        match enchant_count {
            1 => Color32::from_rgb(0, 255, 0),   // Green
            2 => Color32::from_rgb(0, 200, 255), // Blue
            3 => Color32::from_rgb(200, 0, 255), // Purple
            _ => Color32::from_rgb(255, 215, 0), // Gold
        }
    }

    /// Extract a sprite from the atlas as an RgbaImage.
    /// If the item has a dyed texture in the cache, returns that instead.
    /// Bake a 1px black contour outline around an already-integer-scaled RGBA
    /// sprite. Returns a new image padded by 1px on each side: transparent
    /// pixels adjacent to an opaque pixel become black, then the sprite is
    /// stamped on top (offset by 1,1).
    fn bake_outline(scaled: &RgbaImage) -> RgbaImage {
        Self::bake_outline_color(scaled, [0, 0, 0])
    }

    /// Like [`Self::bake_outline`] but the 1px contour uses `outline` (RGB)
    /// instead of black. Used to draw a colour-matched border around secret-stat
    /// glow sprites so the halo colour reads even against a dark row.
    fn bake_outline_color(scaled: &RgbaImage, outline: [u8; 3]) -> RgbaImage {
        let ssw = scaled.width();
        let ssh = scaled.height();
        let ow = ssw + 2;
        let oh = ssh + 2;
        let mut out = RgbaImage::new(ow, oh);

        for y in 0..oh {
            for x in 0..ow {
                let sx = x as i32 - 1;
                let sy = y as i32 - 1;

                let src_alpha = if sx >= 0 && sy >= 0 && (sx as u32) < ssw && (sy as u32) < ssh {
                    scaled.get_pixel(sx as u32, sy as u32)[3]
                } else {
                    0
                };

                if src_alpha > 0 {
                    continue;
                }

                let mut has_opaque_neighbor = false;
                for &(dx, dy) in &[
                    (-1i32, 0i32),
                    (1, 0),
                    (0, -1),
                    (0, 1),
                    (-1, -1),
                    (1, -1),
                    (-1, 1),
                    (1, 1),
                ] {
                    let nx = sx + dx;
                    let ny = sy + dy;
                    if nx >= 0 && ny >= 0 && (nx as u32) < ssw && (ny as u32) < ssh {
                        if scaled.get_pixel(nx as u32, ny as u32)[3] > 0 {
                            has_opaque_neighbor = true;
                            break;
                        }
                    }
                }

                if has_opaque_neighbor {
                    out.put_pixel(x, y, Rgba([outline[0], outline[1], outline[2], 255]));
                }
            }
        }

        for y in 0..ssh {
            for x in 0..ssw {
                let p = scaled.get_pixel(x, y);
                if p[3] > 0 {
                    out.put_pixel(x + 1, y + 1, *p);
                }
            }
        }

        out
    }

    fn extract_sprite_image(&self, item_id: i32) -> Option<RgbaImage> {
        // Return dye-composited image if available
        if let Some(entry) = self.dye_cache.get(&item_id) {
            return Some(entry.image.clone());
        }

        let sprite_data = get_asset_manager().get_object_sprite(item_id)?;

        if sprite_data.width < 4 || sprite_data.height < 4 {
            return None;
        }

        let atlas = self.atlas_images.get(&sprite_data.atlas_id)?;

        Self::crop_atlas_region(
            sprite_data.atlas_id,
            atlas,
            sprite_data.x,
            sprite_data.y,
            sprite_data.width,
            sprite_data.height,
        )
    }

    /// Bake a sprite with a 1px black outline at a given integer scale.
    /// The output is the sprite scaled up by `scale` with a 1px border of
    /// black around the opaque region (dilated at the scaled resolution).
    fn generate_outlined_image(&self, item_id: i32, scale: u32) -> Option<RgbaImage> {
        // Textile-pattern dyes composite directly at display scale so the 1px
        // outline is baked at final resolution (crisp), matching normal sprites.
        // Everything else uses the native sprite/solid-dye image scaled by `scale`.
        let scaled = if let Some(img) = self.composite_textile_dye(item_id, scale) {
            img
        } else {
            let src = self.extract_sprite_image(item_id)?;
            let sw = src.width();
            let sh = src.height();
            image::imageops::resize(
                &src,
                sw * scale,
                sh * scale,
                image::imageops::FilterType::Nearest,
            )
        };
        let ssw = scaled.width();
        let ssh = scaled.height();

        if ssw == 0 || ssh == 0 {
            return None;
        }

        Some(Self::bake_outline(&scaled))
    }

    /// Ensure the outlined cache has a baked texture for this item at the given scale.
    fn ensure_outlined_cache(&mut self, ctx: &egui::Context, item_id: i32, scale: u32) -> bool {
        let key = (item_id, scale);
        if self.outlined_cache.contains_key(&key) {
            return true;
        }

        // Don't cache if item has dye data but dye cache isn't populated yet
        // (atlas may still be loading — we'd bake an undyed sprite permanently)
        if !self.dye_cache.contains_key(&item_id)
            && get_asset_manager().get_item_dye_info(item_id).is_some()
        {
            return false;
        }

        if let Some(image) = self.generate_outlined_image(item_id, scale) {
            let (w, h) = (image.width(), image.height());
            let color_image =
                ColorImage::from_rgba_unmultiplied([w as usize, h as usize], image.as_raw());
            let texture = ctx.load_texture(
                format!("outlined_{}_{}x", item_id, scale),
                color_image,
                TextureOptions::NEAREST,
            );
            self.outlined_cache.insert(key, texture);
            true
        } else {
            false
        }
    }

    /// Extract a direction-aware sprite from the atlas as an RgbaImage.
    /// If a char_dye_cache entry exists for this key, uses that instead.
    fn extract_directional_sprite_image(
        &self,
        sprite_id: i32,
        direction: i32,
        tex1: u32,
        tex2: u32,
    ) -> Option<RgbaImage> {
        let key: CharDyeCacheKey = (sprite_id, direction, tex1, tex2);
        if let Some(entry) = self.char_dye_cache.get(&key) {
            return Some(entry.image.clone());
        }

        let sprite_data =
            get_asset_manager().get_object_sprite_with_direction(sprite_id, direction)?;
        if sprite_data.width < 4 || sprite_data.height < 4 {
            return None;
        }
        let atlas = self.atlas_images.get(&sprite_data.atlas_id)?;
        Self::crop_atlas_region(
            sprite_data.atlas_id,
            atlas,
            sprite_data.x,
            sprite_data.y,
            sprite_data.width,
            sprite_data.height,
        )
    }

    /// Bake a direction-aware sprite with a 1px outline at `scale`. `outline` is
    /// the RGB colour of the contour (black for normal sprites).
    fn generate_outlined_dir_image(
        &self,
        sprite_id: i32,
        direction: i32,
        scale: u32,
        tex1: u32,
        tex2: u32,
        outline: [u8; 3],
    ) -> Option<RgbaImage> {
        let src = self.extract_directional_sprite_image(sprite_id, direction, tex1, tex2)?;

        // Target size based on native sprite dimensions to avoid double-scaling
        // when the dye cache returns an already-upscaled image (textile patterns)
        let native = get_asset_manager().get_object_sprite_with_direction(sprite_id, direction)?;
        let target_w = native.width as u32 * scale;
        let target_h = native.height as u32 * scale;

        let scaled = if src.width() == target_w && src.height() == target_h {
            src
        } else {
            image::imageops::resize(
                &src,
                target_w,
                target_h,
                image::imageops::FilterType::Nearest,
            )
        };
        let ssw = scaled.width();
        let ssh = scaled.height();

        if ssw == 0 || ssh == 0 {
            return None;
        }

        Some(Self::bake_outline_color(&scaled, outline))
    }

    /// Ensure the direction-aware outlined cache has a baked texture.
    fn ensure_outlined_dir_cache(
        &mut self,
        ctx: &egui::Context,
        sprite_id: i32,
        direction: i32,
        scale: u32,
        tex1: u32,
        tex2: u32,
    ) -> bool {
        let key = (sprite_id, direction, scale, tex1, tex2);
        if self.outlined_dir_cache.contains_key(&key) {
            return true;
        }

        // Don't cache if dyes are requested but char_dye_cache isn't populated
        // (mask atlas may still be loading)
        if (tex1 != 0 || tex2 != 0)
            && !self
                .char_dye_cache
                .contains_key(&(sprite_id, direction, tex1, tex2))
        {
            return false;
        }

        if let Some(image) =
            self.generate_outlined_dir_image(sprite_id, direction, scale, tex1, tex2, [0, 0, 0])
        {
            let (w, h) = (image.width(), image.height());
            let color_image =
                ColorImage::from_rgba_unmultiplied([w as usize, h as usize], image.as_raw());
            let texture = ctx.load_texture(
                format!("outlined_dir_{}_{}_{}x", sprite_id, direction, scale),
                color_image,
                TextureOptions::NEAREST,
            );
            self.outlined_dir_cache.insert(key, texture);
            true
        } else {
            false
        }
    }

    /// Ensure the direction-aware *colour-outlined* cache has a baked texture
    /// whose contour is `outline` instead of black.
    fn ensure_outlined_dir_glow_cache(
        &mut self,
        ctx: &egui::Context,
        sprite_id: i32,
        direction: i32,
        scale: u32,
        tex1: u32,
        tex2: u32,
        outline: Color32,
    ) -> bool {
        let rgb = ((outline.r() as u32) << 16) | ((outline.g() as u32) << 8) | (outline.b() as u32);
        let key = (sprite_id, direction, scale, tex1, tex2, rgb);
        if self.outlined_dir_glow_cache.contains_key(&key) {
            return true;
        }

        if (tex1 != 0 || tex2 != 0)
            && !self
                .char_dye_cache
                .contains_key(&(sprite_id, direction, tex1, tex2))
        {
            return false;
        }

        if let Some(image) = self.generate_outlined_dir_image(
            sprite_id,
            direction,
            scale,
            tex1,
            tex2,
            [outline.r(), outline.g(), outline.b()],
        ) {
            let (w, h) = (image.width(), image.height());
            let color_image =
                ColorImage::from_rgba_unmultiplied([w as usize, h as usize], image.as_raw());
            let texture = ctx.load_texture(
                format!(
                    "outlined_dir_glow_{}_{}_{}x_{:06X}",
                    sprite_id, direction, scale, rgb
                ),
                color_image,
                TextureOptions::NEAREST,
            );
            self.outlined_dir_glow_cache.insert(key, texture);
            true
        } else {
            false
        }
    }

    /// Generate a dye-composited image for items with mask + fill data.
    ///
    /// - **Solid-color dyes**: the dye RGB is blended into the mask area,
    ///   preserving the base sprite's luminance for shading.
    /// - **Textile patterns**: the pattern sprite is tiled across the mask
    ///   area, clipped to the base sprite's alpha.
    fn generate_dyed_image(&self, item_id: i32) -> Option<RgbaImage> {
        use realmhound_core::assets::DyeStyle;

        let mgr = get_asset_manager();
        let dye_info = mgr.get_item_dye_info(item_id)?;

        // Extract base sprite from atlas
        let sprite_data = mgr.get_object_sprite(item_id)?;
        let atlas = self.atlas_images.get(&sprite_data.atlas_id)?;
        let base = Self::crop_atlas_region(
            sprite_data.atlas_id,
            atlas,
            sprite_data.x,
            sprite_data.y,
            sprite_data.width,
            sprite_data.height,
        )?;

        // Extract mask sprite from atlas
        let mask_atlas = self.atlas_images.get(&dye_info.mask_sprite.atlas_id)?;
        let mask = Self::crop_atlas_region(
            dye_info.mask_sprite.atlas_id,
            mask_atlas,
            dye_info.mask_sprite.x,
            dye_info.mask_sprite.y,
            dye_info.mask_sprite.width,
            dye_info.mask_sprite.height,
        )?;

        let mut result = base.clone();
        let w = base.width().min(mask.width());
        let h = base.height().min(mask.height());
        if w == 0 || h == 0 {
            return None;
        }

        match dye_info.style {
            DyeStyle::SolidColor(cr, cg, cb) => {
                for y in 0..h {
                    for x in 0..w {
                        let mp = mask.get_pixel(x, y);
                        if mp[3] > 0 {
                            let bp = base.get_pixel(x, y);
                            if bp[3] > 0 {
                                // Use mask alpha as brightness
                                let intensity = mp[3] as f32 / 255.0;
                                result.put_pixel(
                                    x,
                                    y,
                                    Rgba([
                                        (cr as f32 * intensity) as u8,
                                        (cg as f32 * intensity) as u8,
                                        (cb as f32 * intensity) as u8,
                                        bp[3],
                                    ]),
                                );
                            }
                        }
                    }
                }
            }
            DyeStyle::TextilePattern(_) => {
                // Composite at a fixed upscale for the dye cache (used by the
                // non-outlined draw path). The outlined tile path composites at
                // display scale instead - see generate_outlined_image.
                let up = (40u32 / w.max(h)).max(2);
                return self.composite_textile_dye(item_id, up);
            }
        }

        Some(result)
    }

    /// Composite a textile-pattern dye at integer upscale `up`: the base + mask
    /// are nearest-scaled to `native * up` and the pattern is tiled across the
    /// masked area at native resolution. Returns `None` for non-textile items.
    fn composite_textile_dye(&self, item_id: i32, up: u32) -> Option<RgbaImage> {
        use realmhound_core::assets::DyeStyle;

        let mgr = get_asset_manager();
        let dye_info = mgr.get_item_dye_info(item_id)?;
        let pat_sprite = match &dye_info.style {
            DyeStyle::TextilePattern(p) => p,
            _ => return None,
        };

        let sprite_data = mgr.get_object_sprite(item_id)?;
        let atlas = self.atlas_images.get(&sprite_data.atlas_id)?;
        let base = Self::crop_atlas_region(
            sprite_data.atlas_id,
            atlas,
            sprite_data.x,
            sprite_data.y,
            sprite_data.width,
            sprite_data.height,
        )?;

        let mask_atlas = self.atlas_images.get(&dye_info.mask_sprite.atlas_id)?;
        let mask = Self::crop_atlas_region(
            dye_info.mask_sprite.atlas_id,
            mask_atlas,
            dye_info.mask_sprite.x,
            dye_info.mask_sprite.y,
            dye_info.mask_sprite.width,
            dye_info.mask_sprite.height,
        )?;

        let pat_atlas = self.atlas_images.get(&pat_sprite.atlas_id)?;
        let pattern = Self::crop_atlas_region(
            pat_sprite.atlas_id,
            pat_atlas,
            pat_sprite.x,
            pat_sprite.y,
            pat_sprite.width,
            pat_sprite.height,
        )?;
        let pw = pattern.width();
        let ph = pattern.height();

        let w = base.width().min(mask.width());
        let h = base.height().min(mask.height());
        if pw == 0 || ph == 0 || w == 0 || h == 0 {
            return None;
        }

        let up = up.max(1);
        let cw = w * up;
        let ch = h * up;

        let scaled_base =
            image::imageops::resize(&base, cw, ch, image::imageops::FilterType::Nearest);
        let scaled_mask =
            image::imageops::resize(&mask, cw, ch, image::imageops::FilterType::Nearest);

        let mut canvas = scaled_base.clone();
        for cy in 0..ch {
            for cx in 0..cw {
                if scaled_mask.get_pixel(cx, cy)[3] > 0 {
                    let bp = scaled_base.get_pixel(cx, cy);
                    if bp[3] > 0 {
                        let pp = pattern.get_pixel(cx % pw, cy % ph);
                        canvas.put_pixel(cx, cy, Rgba([pp[0], pp[1], pp[2], bp[3]]));
                    }
                }
            }
        }

        Some(canvas)
    }

    /// Ensure the dye cache contains a composited texture for this item
    /// (if it has dye data).  Returns `true` if a dyed texture is available.
    fn ensure_dye_cache(&mut self, ctx: &egui::Context, item_id: i32) -> bool {
        if self.dye_cache.contains_key(&item_id) {
            return true;
        }

        if let Some(image) = self.generate_dyed_image(item_id) {
            let (w, h) = (image.width(), image.height());
            let color_image =
                ColorImage::from_rgba_unmultiplied([w as usize, h as usize], image.as_raw());
            let texture = ctx.load_texture(
                format!("dye_{}", item_id),
                color_image,
                TextureOptions::NEAREST,
            );
            self.dye_cache.insert(
                item_id,
                DyeCacheEntry {
                    texture,
                    image,
                    width: w,
                    height: h,
                },
            );
            true
        } else {
            false
        }
    }

    /// Generate a dye-composited image for a character sprite.
    ///
    /// Character masks use three channels:
    /// - **Red + Alpha** (r > 0 && a > 0): large cloth / clothing region, filled with tex1
    /// - **Green + Alpha** (g > 0 && a > 0): small cloth / accessory region, filled with tex2
    /// - Alpha = 0 means "don't dye" regardless of R/G values.
    ///
    /// Both solid-color dyes and textile patterns are supported, same as items.
    fn generate_character_dyed_image(
        &self,
        sprite_id: i32,
        direction: i32,
        tex1: u32,
        tex2: u32,
    ) -> Option<RgbaImage> {
        use realmhound_core::assets::DyeStyle;

        let mgr = get_asset_manager();
        let dye_info = mgr.get_character_dye_info(sprite_id, direction, tex1, tex2)?;

        // Get the base character sprite
        let sprite_data = mgr.get_object_sprite_with_direction(sprite_id, direction)?;
        let atlas = self.atlas_images.get(&sprite_data.atlas_id)?;
        let base = Self::crop_atlas_region(
            sprite_data.atlas_id,
            atlas,
            sprite_data.x,
            sprite_data.y,
            sprite_data.width,
            sprite_data.height,
        )?;

        // Get the mask from characters_masks atlas (atlas 3)
        let mask_atlas = self.atlas_images.get(&dye_info.mask_sprite.atlas_id)?;
        let mask = Self::crop_atlas_region(
            dye_info.mask_sprite.atlas_id,
            mask_atlas,
            dye_info.mask_sprite.x,
            dye_info.mask_sprite.y,
            dye_info.mask_sprite.width,
            dye_info.mask_sprite.height,
        )?;

        let w = base.width().min(mask.width());
        let h = base.height().min(mask.height());
        if w == 0 || h == 0 {
            return None;
        }

        // Check if any channel needs textile upscaling
        let needs_textile = matches!(&dye_info.clothing, Some(DyeStyle::TextilePattern(_)))
            || matches!(&dye_info.accessory, Some(DyeStyle::TextilePattern(_)));

        if needs_textile {
            // Upscale for tiled textile patterns (same approach as item dyes)
            let target_size = 40u32;
            let up = (target_size / w.max(h)).max(2);
            let cw = w * up;
            let ch = h * up;

            let scaled_base =
                image::imageops::resize(&base, cw, ch, image::imageops::FilterType::Nearest);
            let scaled_mask =
                image::imageops::resize(&mask, cw, ch, image::imageops::FilterType::Nearest);

            // Load textile pattern tiles if needed
            let clothing_pattern =
                if let Some(DyeStyle::TextilePattern(ref pat)) = dye_info.clothing {
                    let pat_atlas = self.atlas_images.get(&pat.atlas_id)?;
                    Self::crop_atlas_region(
                        pat.atlas_id,
                        pat_atlas,
                        pat.x,
                        pat.y,
                        pat.width,
                        pat.height,
                    )
                } else {
                    None
                };
            let accessory_pattern =
                if let Some(DyeStyle::TextilePattern(ref pat)) = dye_info.accessory {
                    let pat_atlas = self.atlas_images.get(&pat.atlas_id)?;
                    Self::crop_atlas_region(
                        pat.atlas_id,
                        pat_atlas,
                        pat.x,
                        pat.y,
                        pat.width,
                        pat.height,
                    )
                } else {
                    None
                };

            let mut canvas = scaled_base.clone();

            for cy in 0..ch {
                for cx in 0..cw {
                    let mp = scaled_mask.get_pixel(cx, cy);
                    let bp = scaled_base.get_pixel(cx, cy);
                    if bp[3] == 0 {
                        continue;
                    }

                    // Use the dominant channel only (red vs green).
                    // Threshold > 30 filters PNG compression artifacts (real values are 133+)
                    if mp[3] > 0 {
                        let red = mp[0];
                        let green = mp[1];

                        if red > green && red > 30 {
                            // Clothing (tex1) - red channel dominant
                            if let Some(ref pat) = clothing_pattern {
                                let pw = pat.width();
                                let ph = pat.height();
                                if pw > 0 && ph > 0 {
                                    let pp = pat.get_pixel(cx % pw, cy % ph);
                                    canvas.put_pixel(cx, cy, Rgba([pp[0], pp[1], pp[2], bp[3]]));
                                }
                            } else if let Some(DyeStyle::SolidColor(cr, cg, cb)) = dye_info.clothing
                            {
                                let intensity = red as f32 / 255.0;
                                canvas.put_pixel(
                                    cx,
                                    cy,
                                    Rgba([
                                        (cr as f32 * intensity) as u8,
                                        (cg as f32 * intensity) as u8,
                                        (cb as f32 * intensity) as u8,
                                        bp[3],
                                    ]),
                                );
                            }
                        } else if green > red && green > 30 {
                            // Accessory (tex2) - green channel dominant
                            if let Some(ref pat) = accessory_pattern {
                                let pw = pat.width();
                                let ph = pat.height();
                                if pw > 0 && ph > 0 {
                                    let pp = pat.get_pixel(cx % pw, cy % ph);
                                    canvas.put_pixel(cx, cy, Rgba([pp[0], pp[1], pp[2], bp[3]]));
                                }
                            } else if let Some(DyeStyle::SolidColor(cr, cg, cb)) =
                                dye_info.accessory
                            {
                                let intensity = green as f32 / 255.0;
                                canvas.put_pixel(
                                    cx,
                                    cy,
                                    Rgba([
                                        (cr as f32 * intensity) as u8,
                                        (cg as f32 * intensity) as u8,
                                        (cb as f32 * intensity) as u8,
                                        bp[3],
                                    ]),
                                );
                            }
                        }
                    }
                }
            }
            return Some(canvas);
        }

        // No textile patterns - work at native resolution
        let mut result = base.clone();

        for y in 0..h {
            for x in 0..w {
                let mp = mask.get_pixel(x, y);
                let bp = base.get_pixel(x, y);
                if bp[3] == 0 {
                    continue;
                }

                // Use the dominant channel only (red vs green)
                if mp[3] > 0 {
                    let red = mp[0];
                    let green = mp[1];

                    if red > green && red > 30 {
                        // Clothing (tex1) - red channel dominant
                        if let Some(DyeStyle::SolidColor(cr, cg, cb)) = dye_info.clothing {
                            let intensity = red as f32 / 255.0;
                            result.put_pixel(
                                x,
                                y,
                                Rgba([
                                    (cr as f32 * intensity) as u8,
                                    (cg as f32 * intensity) as u8,
                                    (cb as f32 * intensity) as u8,
                                    bp[3],
                                ]),
                            );
                        }
                    } else if green > red && green > 30 {
                        // Accessory (tex2) - green channel dominant
                        if let Some(DyeStyle::SolidColor(cr, cg, cb)) = dye_info.accessory {
                            let intensity = green as f32 / 255.0;
                            result.put_pixel(
                                x,
                                y,
                                Rgba([
                                    (cr as f32 * intensity) as u8,
                                    (cg as f32 * intensity) as u8,
                                    (cb as f32 * intensity) as u8,
                                    bp[3],
                                ]),
                            );
                        }
                    }
                }
            }
        }

        Some(result)
    }

    /// Ensure the character dye cache contains a composited texture for this
    /// sprite+direction+tex combination.  Returns `true` if available.
    fn ensure_char_dye_cache(
        &mut self,
        ctx: &egui::Context,
        sprite_id: i32,
        direction: i32,
        tex1: u32,
        tex2: u32,
    ) -> bool {
        let key: CharDyeCacheKey = (sprite_id, direction, tex1, tex2);
        if self.char_dye_cache.contains_key(&key) {
            return true;
        }

        if let Some(image) = self.generate_character_dyed_image(sprite_id, direction, tex1, tex2) {
            let (w, h) = (image.width(), image.height());
            let color_image =
                ColorImage::from_rgba_unmultiplied([w as usize, h as usize], image.as_raw());
            let texture = ctx.load_texture(
                format!(
                    "char_dye_{}_{}_{:08x}_{:08x}",
                    sprite_id, direction, tex1, tex2
                ),
                color_image,
                TextureOptions::NEAREST,
            );
            self.char_dye_cache.insert(
                key,
                DyeCacheEntry {
                    texture,
                    image,
                    width: w,
                    height: h,
                },
            );
            true
        } else {
            false
        }
    }

    /// Draw a character sprite with dye/cloth compositing and a contour outline.
    ///
    /// Combines dye compositing with the 1px outline border used in loot feed.
    pub fn draw_dyed_outlined_character_sprite(
        &mut self,
        ui: &egui::Ui,
        sprite_id: i32,
        rect: Rect,
        direction: i32,
        tex1: u32,
        tex2: u32,
    ) -> bool {
        self.draw_dyed_outlined_character_sprite_overlay(
            ui, sprite_id, rect, direction, tex1, tex2, None,
        )
    }

    /// Same as [`draw_dyed_outlined_character_sprite`] but paints a second,
    /// silhouette-masked pass tinted with `overlay` on top (draws a
    /// #ff0000 @50% "close call" overlay this way). The image tint multiplies the
    /// texture, so transparent pixels stay transparent and only the sprite body
    /// takes the colour.
    pub fn draw_dyed_outlined_character_sprite_overlay(
        &mut self,
        ui: &egui::Ui,
        sprite_id: i32,
        rect: Rect,
        direction: i32,
        tex1: u32,
        tex2: u32,
        overlay: Option<Color32>,
    ) -> bool {
        // Ensure char dye cache if dyes are set
        if tex1 != 0 || tex2 != 0 {
            self.ensure_char_dye_cache(ui.ctx(), sprite_id, direction, tex1, tex2);
        }

        let sprite_data =
            match get_asset_manager().get_object_sprite_with_direction(sprite_id, direction) {
                Some(data) => data,
                None => return false,
            };
        if sprite_data.width < 4 || sprite_data.height < 4 {
            return false;
        }

        let dpi_scale = ui.ctx().pixels_per_point();
        let sprite_w = sprite_data.width as f32;
        let sprite_h = sprite_data.height as f32;
        let physical_rect_w = rect.width() * dpi_scale;
        let physical_rect_h = rect.height() * dpi_scale;
        let raw_scale = (physical_rect_w / sprite_w).min(physical_rect_h / sprite_h);
        let scale = if raw_scale >= 1.0 {
            raw_scale.floor() as u32
        } else {
            1
        };
        let scale = scale.max(1);

        if self.ensure_outlined_dir_cache(ui.ctx(), sprite_id, direction, scale, tex1, tex2) {
            let texture = self
                .outlined_dir_cache
                .get(&(sprite_id, direction, scale, tex1, tex2))
                .unwrap();
            let draw_rect = Self::centered_rect_for_texture(ui, texture, rect, egui::Vec2::ZERO);
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            ui.painter()
                .image(texture.id(), draw_rect, uv, Color32::WHITE);
            if let Some(tint) = overlay {
                ui.painter().image(texture.id(), draw_rect, uv, tint);
            }
            count_draw_call();
            count_outlined_sprite();
            return true;
        }

        false
    }

    /// Draw a dyed directional character sprite with a soft colored glow halo
    /// behind it (gold for "Lone fighter", purple for "Last hero standing").
    /// Mirrors the enchant weapon-glow look but built from the character
    /// silhouette. Returns whether the sprite was drawn.
    pub fn draw_dyed_outlined_character_sprite_glow(
        &mut self,
        ui: &egui::Ui,
        sprite_id: i32,
        rect: Rect,
        direction: i32,
        tex1: u32,
        tex2: u32,
        glow_color: Color32,
        glow_size: u8,
        overlay: Option<Color32>,
    ) -> bool {
        if tex1 != 0 || tex2 != 0 {
            self.ensure_char_dye_cache(ui.ctx(), sprite_id, direction, tex1, tex2);
        }

        let sprite_data =
            match get_asset_manager().get_object_sprite_with_direction(sprite_id, direction) {
                Some(data) => data,
                None => return false,
            };
        if sprite_data.width < 4 || sprite_data.height < 4 {
            return false;
        }

        let dpi_scale = ui.ctx().pixels_per_point();
        let sprite_w = sprite_data.width as f32;
        let sprite_h = sprite_data.height as f32;
        let physical_rect_w = rect.width() * dpi_scale;
        let physical_rect_h = rect.height() * dpi_scale;
        let raw_scale = (physical_rect_w / sprite_w).min(physical_rect_h / sprite_h);
        let target_size = (sprite_w.max(sprite_h) * raw_scale.max(1.0)) as u32;

        // Generate + cache the glow halo, keyed by pose + dye + colour so it never
        // collides with an item glow or another character pose.
        let key: CharGlowCacheKey = (
            sprite_id,
            direction,
            tex1,
            tex2,
            ((glow_color.r() as u32) << 16)
                | ((glow_color.g() as u32) << 8)
                | (glow_color.b() as u32),
            glow_size,
            target_size,
        );
        if !self.char_glow_cache.contains_key(&key) {
            if let Some(src) =
                self.extract_directional_sprite_image(sprite_id, direction, tex1, tex2)
            {
                let name = format!(
                    "charglow_{}_{}_{:08X}_{}_{}",
                    sprite_id, direction, key.4, glow_size, target_size
                );
                if let Some(tex) = Self::build_glow_texture(
                    ui.ctx(),
                    &src,
                    glow_color,
                    glow_size,
                    target_size,
                    &name,
                    60.0,
                ) {
                    self.char_glow_cache.insert(key, tex);
                }
            }
        }

        // Draw the glow behind, sized to the sprite's centered draw rect.
        if let Some(texture) = self.char_glow_cache.get(&key) {
            let scale = if raw_scale >= 1.0 {
                raw_scale.floor()
            } else {
                raw_scale
            };
            let scaled_w = (sprite_w * scale) / dpi_scale;
            let scaled_h = (sprite_h * scale) / dpi_scale;
            let sprite_rect = Rect::from_center_size(rect.center(), egui::vec2(scaled_w, scaled_h));
            let padding = glow_size as f32;
            let expanded = sprite_rect.expand(padding);
            ui.painter().image(
                texture.id(),
                expanded,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }

        // Draw the character sprite on top, using a contour outline tinted to the
        // glow colour so the halo reads even against dark rows.
        self.draw_dyed_outlined_character_sprite_colored_outline(
            ui, sprite_id, rect, direction, tex1, tex2, overlay, glow_color,
        )
    }

    /// Same as [`Self::draw_dyed_outlined_character_sprite_overlay`] but the 1px
    /// contour is baked in `outline` instead of black. Used by the secret-stat
    /// glow path to colour-match the sprite border to its halo.
    pub fn draw_dyed_outlined_character_sprite_colored_outline(
        &mut self,
        ui: &egui::Ui,
        sprite_id: i32,
        rect: Rect,
        direction: i32,
        tex1: u32,
        tex2: u32,
        overlay: Option<Color32>,
        outline: Color32,
    ) -> bool {
        if tex1 != 0 || tex2 != 0 {
            self.ensure_char_dye_cache(ui.ctx(), sprite_id, direction, tex1, tex2);
        }

        let sprite_data =
            match get_asset_manager().get_object_sprite_with_direction(sprite_id, direction) {
                Some(data) => data,
                None => return false,
            };
        if sprite_data.width < 4 || sprite_data.height < 4 {
            return false;
        }

        let dpi_scale = ui.ctx().pixels_per_point();
        let sprite_w = sprite_data.width as f32;
        let sprite_h = sprite_data.height as f32;
        let physical_rect_w = rect.width() * dpi_scale;
        let physical_rect_h = rect.height() * dpi_scale;
        let raw_scale = (physical_rect_w / sprite_w).min(physical_rect_h / sprite_h);
        let scale = if raw_scale >= 1.0 {
            raw_scale.floor() as u32
        } else {
            1
        };
        let scale = scale.max(1);

        if self.ensure_outlined_dir_glow_cache(
            ui.ctx(),
            sprite_id,
            direction,
            scale,
            tex1,
            tex2,
            outline,
        ) {
            let rgb =
                ((outline.r() as u32) << 16) | ((outline.g() as u32) << 8) | (outline.b() as u32);
            let texture = self
                .outlined_dir_glow_cache
                .get(&(sprite_id, direction, scale, tex1, tex2, rgb))
                .unwrap();
            let draw_rect = Self::centered_rect_for_texture(ui, texture, rect, egui::Vec2::ZERO);
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            ui.painter()
                .image(texture.id(), draw_rect, uv, Color32::WHITE);
            if let Some(tint) = overlay {
                ui.painter().image(texture.id(), draw_rect, uv, tint);
            }
            count_draw_call();
            count_outlined_sprite();
            return true;
        }

        // Fall back to the black-outline path if the coloured bake isn't ready.
        self.draw_dyed_outlined_character_sprite_overlay(
            ui, sprite_id, rect, direction, tex1, tex2, overlay,
        )
    }

    /// Generate a glowed sprite texture at target display size.
    ///
    /// This replicates ImageBuffer.getOutlinedIconWithGlow():
    /// 1. Scale sprite to target size (pixel-perfect nearest-neighbor)
    /// 2. Detect edges to find sprite contour
    /// 3. Create smooth glow around the edge
    /// 4. Mask glow where sprite has alpha
    ///
    /// Note: Generates ONLY the glow effect (no outline, no sprite).
    /// Outline is drawn separately at render time for thin lines.
    fn generate_glow_texture(
        &self,
        ctx: &egui::Context,
        item_id: i32,
        glow_color: Color32,
        glow_size: u8,
        target_size: u32,
    ) -> Option<TextureHandle> {
        let sprite = self.extract_sprite_image(item_id)?;
        let name = format!(
            "glow_{}_{:08X}_{}_{}",
            item_id,
            ((glow_color.r() as u32) << 16)
                | ((glow_color.g() as u32) << 8)
                | (glow_color.b() as u32),
            glow_size,
            target_size
        );
        Self::build_glow_texture(
            ctx,
            &sprite,
            glow_color,
            glow_size,
            target_size,
            &name,
            60.0,
        )
    }

    /// Build a silhouette glow texture from an arbitrary sprite image. Shared by
    /// the item-glow and character-glow paths (an edge-spread outline). Generates ONLY the glow halo.
    fn build_glow_texture(
        ctx: &egui::Context,
        sprite: &RgbaImage,
        glow_color: Color32,
        glow_size: u8,
        target_size: u32,
        name: &str,
        peak_alpha: f32,
    ) -> Option<TextureHandle> {
        let (native_w, native_h) = (sprite.width(), sprite.height());
        if native_w < 4 || native_h < 4 {
            return None;
        }

        // Scale sprite to target size using nearest-neighbor (pixel-perfect)
        let scale = target_size as f32 / native_w.max(native_h) as f32;
        let scaled_w = (native_w as f32 * scale).round() as u32;
        let scaled_h = (native_h as f32 * scale).round() as u32;

        let scaled_sprite = image::imageops::resize(
            sprite,
            scaled_w,
            scaled_h,
            image::imageops::FilterType::Nearest,
        );

        // Add padding for glow (in display pixels)
        let padding = glow_size as u32;
        let w = scaled_w + padding * 2;
        let h = scaled_h + padding * 2;

        // Step 1: Create base image with scaled sprite centered
        let mut base = RgbaImage::new(w, h);
        for y in 0..scaled_h {
            for x in 0..scaled_w {
                let pixel = scaled_sprite.get_pixel(x, y);
                base.put_pixel(x + padding, y + padding, *pixel);
            }
        }

        // Step 2: Detect edges (where sprite meets transparency)
        let mut edges = RgbaImage::new(w, h);
        for y in 1..(h - 1) {
            for x in 1..(w - 1) {
                let alpha = base.get_pixel(x, y)[3];

                if alpha == 0 {
                    // Check if any neighbor has alpha (is edge)
                    let neighbors = [
                        base.get_pixel(x + 1, y)[3],
                        base.get_pixel(x - 1, y)[3],
                        base.get_pixel(x, y + 1)[3],
                        base.get_pixel(x, y - 1)[3],
                    ];

                    if neighbors.iter().any(|&a| a > 0) {
                        edges.put_pixel(x, y, Rgba([255, 255, 255, 255]));
                    }
                }
            }
        }

        // Step 3: Create glow around the edges
        let mut glow = RgbaImage::new(w, h);
        let glow_r = glow_color.r();
        let glow_g = glow_color.g();
        let glow_b = glow_color.b();
        let glow_size_i = glow_size as i32;

        for y in 0..h {
            for x in 0..w {
                let edge_pixel = edges.get_pixel(x, y);
                if edge_pixel[3] > 0 {
                    // This is an edge pixel - spread glow from here
                    for dy in -glow_size_i..=glow_size_i {
                        for dx in -glow_size_i..=glow_size_i {
                            let nx = x as i32 + dx;
                            let ny = y as i32 + dy;

                            if nx >= 0 && ny >= 0 && (nx as u32) < w && (ny as u32) < h {
                                let dist_sq = dx * dx + dy * dy;
                                let max_dist_sq = glow_size_i * glow_size_i;

                                if dist_sq <= max_dist_sq {
                                    // Smooth alpha falloff based on distance
                                    let dist = (dist_sq as f32).sqrt();
                                    let max_dist = glow_size as f32;
                                    let glow_alpha = ((1.0 - dist / max_dist) * peak_alpha) as u8;

                                    let nx = nx as u32;
                                    let ny = ny as u32;
                                    let existing = glow.get_pixel(nx, ny);
                                    let combined_alpha = existing[3].max(glow_alpha);
                                    glow.put_pixel(
                                        nx,
                                        ny,
                                        Rgba([glow_r, glow_g, glow_b, combined_alpha]),
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        // Step 4: Mask glow where sprite has alpha (glow only outside sprite)
        for y in 0..h {
            for x in 0..w {
                if base.get_pixel(x, y)[3] > 0 {
                    glow.put_pixel(x, y, Rgba([0, 0, 0, 0]));
                }
            }
        }

        // Convert to egui texture
        let color_image =
            ColorImage::from_rgba_unmultiplied([w as usize, h as usize], glow.as_raw());

        let texture = ctx.load_texture(name, color_image, TextureOptions::LINEAR);

        Some(texture)
    }

    /// Draw a glowed sprite in the given rect.
    /// Returns true if drawn successfully.
    ///
    /// Draws item sprite with glow effect:
    /// 1. Glow texture (generated at target size, smooth)
    /// 2. Black outline via offset sprites (thin, at display resolution)
    /// 3. Regular sprite on top
    fn draw_glowed_sprite(
        &mut self,
        ctx: &egui::Context,
        ui: &egui::Ui,
        item_id: i32,
        rect: Rect,
        glow_color: Color32,
        glow_size: u8,
    ) -> bool {
        let sprite_data = match get_asset_manager().get_object_sprite(item_id) {
            Some(data) => data,
            None => return false,
        };

        if !self.has_atlas(sprite_data.atlas_id) {
            return false;
        }

        // Pixel-perfect scaling accounting for DPI
        let dpi_scale = ui.ctx().pixels_per_point();
        let sprite_w = sprite_data.width as f32;
        let sprite_h = sprite_data.height as f32;
        let physical_rect_w = rect.width() * dpi_scale;
        let physical_rect_h = rect.height() * dpi_scale;
        let raw_scale = (physical_rect_w / sprite_w).min(physical_rect_h / sprite_h);
        let physical_scale = if raw_scale >= 1.0 {
            raw_scale.floor()
        } else {
            raw_scale
        };
        let scaled_w = (sprite_w * physical_scale) / dpi_scale;
        let scaled_h = (sprite_h * physical_scale) / dpi_scale;
        let content_off = self.content_offset_display(item_id, ui, physical_scale as u32);
        let sprite_rect =
            Rect::from_center_size(rect.center() + content_off, egui::vec2(scaled_w, scaled_h));

        // Target size for glow generation
        let target_size = (sprite_w * physical_scale) as u32;
        let key: GlowCacheKey = (
            item_id,
            ((glow_color.r() as u32) << 16)
                | ((glow_color.g() as u32) << 8)
                | (glow_color.b() as u32),
            glow_size,
            target_size,
        );

        if !self.glow_cache.contains_key(&key) {
            if let Some(texture) =
                self.generate_glow_texture(ctx, item_id, glow_color, glow_size, target_size)
            {
                self.glow_cache.insert(key, texture);
            }
        }

        // 1. Draw glow texture
        if let Some(texture) = self.glow_cache.get(&key) {
            let padding = glow_size as f32;
            let expanded_rect = sprite_rect.expand(padding);
            ui.painter().image(
                texture.id(),
                expanded_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }

        // 2+3. Draw baked outlined sprite (ensures dye cache internally)
        self.ensure_dye_cache(ctx, item_id);
        let scale = physical_scale as u32;
        if scale > 0 && self.ensure_outlined_cache(ctx, item_id, scale) {
            let outlined_tex = self.outlined_cache.get(&(item_id, scale)).unwrap();
            let draw_rect = Self::centered_rect_for_texture(ui, outlined_tex, rect, content_off);
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            ui.painter()
                .image(outlined_tex.id(), draw_rect, uv, Color32::WHITE);
            count_draw_call();
            count_outlined_sprite();
        }

        true
    }

    /// Render an item tile (fixed 38px) with name resolution, shiny detection,
    /// and stack-count extraction, delegating to [`render_item_slot`] for the
    /// drawing. `raw_name` overrides the name unless it is `None` or starts with
    /// "Unknown (", in which case the name is re-resolved from the asset manager.
    pub fn render_item_tile(
        &mut self,
        ui: &mut egui::Ui,
        item_id: i32,
        raw_name: Option<&str>,
        enchant_ids: &[i32],
    ) -> egui::Response {
        const TILE_SIZE: f32 = 38.0;
        self.render_item_tile_sized(ui, item_id, raw_name, enchant_ids, TILE_SIZE)
    }

    /// Same as `render_item_tile` but with a custom tile size (e.g. for compact layouts).
    pub fn render_item_tile_sized(
        &mut self,
        ui: &mut egui::Ui,
        item_id: i32,
        raw_name: Option<&str>,
        enchant_ids: &[i32],
        size: f32,
    ) -> egui::Response {
        let display_name = match raw_name {
            // Re-resolve if name is unknown or is an unresolved localization key like {textiles.X}
            Some(name)
                if !name.starts_with("Unknown (")
                    && !(name.starts_with('{') && name.ends_with('}')) =>
            {
                name.to_string()
            }
            _ => self.item_name(item_id).unwrap_or_else(|| {
                raw_name
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| format!("Item 0x{:04X}", item_id))
            }),
        };
        let (item_name, stack_count) = Self::extract_stack_from_name(&display_name);
        let is_shiny = self.is_shiny(item_id);

        self.render_item_slot(
            ui,
            item_id,
            &item_name,
            enchant_ids,
            is_shiny,
            size,
            stack_count,
        )
    }

    /// Flexible item tile renderer: `highlight` paints the bright-yellow search
    /// background, `clickable` makes the tile respond to clicks (click-to-filter).
    pub fn render_item_tile_ex(
        &mut self,
        ui: &mut egui::Ui,
        item_id: i32,
        raw_name: Option<&str>,
        enchant_ids: &[i32],
        highlight: bool,
        clickable: bool,
    ) -> egui::Response {
        let display_name = match raw_name {
            Some(name)
                if !name.starts_with("Unknown (")
                    && !(name.starts_with('{') && name.ends_with('}')) =>
            {
                name.to_string()
            }
            _ => self.item_name(item_id).unwrap_or_else(|| {
                raw_name
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| format!("Item 0x{:04X}", item_id))
            }),
        };
        let (item_name, stack_count) = Self::extract_stack_from_name(&display_name);
        let is_shiny = self.is_shiny(item_id);

        self.render_item_slot_ex(
            ui,
            item_id,
            &item_name,
            enchant_ids,
            is_shiny,
            38.0,
            stack_count,
            highlight,
            clickable,
        )
    }
    /// Same as `render_item_tile` but responds to clicks.
    pub fn render_item_tile_clickable(
        &mut self,
        ui: &mut egui::Ui,
        item_id: i32,
        raw_name: Option<&str>,
        enchant_ids: &[i32],
    ) -> egui::Response {
        self.render_item_tile_clickable_sized(ui, item_id, raw_name, enchant_ids, 38.0)
    }

    /// Render a clickable item tile at a custom size.
    pub fn render_item_tile_clickable_sized(
        &mut self,
        ui: &mut egui::Ui,
        item_id: i32,
        raw_name: Option<&str>,
        enchant_ids: &[i32],
        size: f32,
    ) -> egui::Response {
        let display_name = match raw_name {
            Some(name)
                if !name.starts_with("Unknown (")
                    && !(name.starts_with('{') && name.ends_with('}')) =>
            {
                name.to_string()
            }
            _ => self.item_name(item_id).unwrap_or_else(|| {
                raw_name
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| format!("Item 0x{:04X}", item_id))
            }),
        };
        let (item_name, stack_count) = Self::extract_stack_from_name(&display_name);
        let is_shiny = self.is_shiny(item_id);

        self.render_item_slot_with_sense(
            ui,
            item_id,
            &item_name,
            enchant_ids,
            is_shiny,
            size,
            stack_count,
            egui::Sense::click(),
            false,
        )
    }

    pub fn render_item_slot(
        &mut self,
        ui: &mut egui::Ui,
        item_id: i32,
        item_name: &str,
        enchant_ids: &[i32],
        is_shiny: bool,
        size: f32,
        stack_count: u8,
    ) -> egui::Response {
        self.render_item_slot_with_sense(
            ui,
            item_id,
            item_name,
            enchant_ids,
            is_shiny,
            size,
            stack_count,
            egui::Sense::hover(),
            false,
        )
    }

    /// Flexible item slot renderer: `highlight` paints the bright-yellow search
    /// background, `clickable` makes the slot respond to clicks (for click-to-filter).
    pub fn render_item_slot_ex(
        &mut self,
        ui: &mut egui::Ui,
        item_id: i32,
        item_name: &str,
        enchant_ids: &[i32],
        is_shiny: bool,
        size: f32,
        stack_count: u8,
        highlight: bool,
        clickable: bool,
    ) -> egui::Response {
        let sense = if clickable {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        self.render_item_slot_with_sense(
            ui,
            item_id,
            item_name,
            enchant_ids,
            is_shiny,
            size,
            stack_count,
            sense,
            highlight,
        )
    }

    fn render_item_slot_with_sense(
        &mut self,
        ui: &mut egui::Ui,
        item_id: i32,
        item_name: &str,
        enchant_ids: &[i32],
        is_shiny: bool,
        size: f32,
        stack_count: u8,
        sense: egui::Sense,
        highlight: bool,
    ) -> egui::Response {
        // Use auto-incrementing ID to ensure each slot is unique within its parent scope
        let slot_id = ui.next_auto_id();
        ui.push_id(slot_id, |ui| {
            self.render_item_slot_inner(
                ui,
                item_id,
                item_name,
                enchant_ids,
                is_shiny,
                size,
                stack_count,
                sense,
                highlight,
            )
        })
        .inner
    }

    fn render_item_slot_inner(
        &mut self,
        ui: &mut egui::Ui,
        item_id: i32,
        item_name: &str,
        enchant_ids: &[i32],
        is_shiny: bool,
        size: f32,
        stack_count: u8,
        sense: egui::Sense,
        highlight: bool,
    ) -> egui::Response {
        self.render_item_slot_inner_impl(
            ui,
            item_id,
            item_name,
            enchant_ids,
            is_shiny,
            size,
            stack_count,
            sense,
            true,
            highlight,
        )
    }

    fn render_item_slot_inner_no_tooltip(
        &mut self,
        ui: &mut egui::Ui,
        item_id: i32,
        item_name: &str,
        enchant_ids: &[i32],
        is_shiny: bool,
        size: f32,
        stack_count: u8,
        sense: egui::Sense,
    ) -> egui::Response {
        self.render_item_slot_inner_impl(
            ui,
            item_id,
            item_name,
            enchant_ids,
            is_shiny,
            size,
            stack_count,
            sense,
            false,
            false,
        )
    }

    fn render_item_slot_inner_impl(
        &mut self,
        ui: &mut egui::Ui,
        item_id: i32,
        item_name: &str,
        enchant_ids: &[i32],
        is_shiny: bool,
        size: f32,
        stack_count: u8,
        sense: egui::Sense,
        show_tooltip: bool,
        highlight: bool,
    ) -> egui::Response {
        let enchant_count = enchant_ids.len();

        // Allocate space for the item
        let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), sense);

        if ui.is_rect_visible(rect) {
            let ui_visuals = ui.visuals();
            let painter = ui.painter();
            let ctx = ui.ctx().clone();

            // Draw background: bright yellow when highlighted (search match),
            // dark otherwise.
            if highlight {
                painter.rect_filled(rect, 4.0, Color32::from_rgb(255, 214, 0));
                painter.rect_stroke(
                    rect,
                    4.0,
                    egui::Stroke::new(1.5_f32, Color32::from_rgb(255, 176, 0)),
                    egui::StrokeKind::Outside,
                );
            } else {
                painter.rect_filled(rect, 4.0, crate::ui_colors::slot_fill(ui_visuals));
                painter.rect_stroke(
                    rect,
                    4.0,
                    egui::Stroke::new(1.0_f32, crate::ui_colors::slot_stroke(ui_visuals)),
                    egui::StrokeKind::Outside,
                );
            }

            // Draw sprite with glow effect for enchanted items
            let inner_rect = rect.shrink(3.0);

            // Pre-generate dyed texture for dye/cloth items (if applicable)
            self.ensure_dye_cache(&ctx, item_id);

            // For enchanted items, draw the pre-generated glow texture (includes glow + outline + sprite)
            let sprite_drawn = if enchant_count > 0 {
                let glow_color = Self::enchant_glow_color(enchant_count);
                let glow_size = 7u8;
                self.draw_glowed_sprite(&ctx, ui, item_id, inner_rect, glow_color, glow_size)
            } else {
                // No enchants - draw sprite with contour outline at native
                // frame scale so padded art (potions, blueprints) renders at a
                // consistent proportion instead of filling the cell.
                self.draw_outlined_sprite_in_rect_native(ui, item_id, inner_rect)
            };

            // Fallback: draw item ID if sprite couldn't be drawn
            if !sprite_drawn {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("{:X}", item_id),
                    egui::FontId::proportional(if size > 30.0 {
                        10.0
                    } else if size > 24.0 {
                        8.0
                    } else {
                        6.0
                    }),
                    Color32::WHITE,
                );
            }

            // Draw shiny overlay in top-left corner (1/4 of slot size, pixel-perfect)
            if is_shiny {
                if let Some(shiny_tex) = &self.shiny_texture {
                    // Pixel-perfect scaling: shiny sprite is 50x50, target is 1/4 slot size
                    let dpi_scale = ui.ctx().pixels_per_point();
                    let target_logical = size / 4.0;
                    let target_physical = target_logical * dpi_scale;
                    // Round to nearest whole physical pixel for crisp rendering
                    let scaled_physical = target_physical.round();
                    let overlay_size = scaled_physical / dpi_scale;

                    let shiny_rect =
                        Rect::from_min_size(rect.left_top(), Vec2::new(overlay_size, overlay_size));
                    painter.image(
                        shiny_tex.id(),
                        shiny_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
            }

            // Blueprint: stamp the unlocked item's icon in the top-left corner.
            self.draw_blueprint_unlock_overlay(ui, item_id, rect);

            // Draw enchant overlay in bottom-right corner (pixel-perfect integer scaling)
            if enchant_count > 0 {
                // Select texture based on enchant count: 1=Uncommon, 2=Rare, 3=Legendary, 4+=Divine
                let tex_idx = (enchant_count.min(4) - 1) as usize;
                if let Some(enchant_tex) = &self.enchant_textures[tex_idx] {
                    // Pixel-perfect scaling: enchant sprites are 16x16.
                    // To avoid uneven pixel mapping, the physical size must be
                    // an exact integer multiple of 16 (1x=16px, 2x=32px, etc.).
                    let sprite_native = 16.0_f32;
                    let dpi_scale = ui.ctx().pixels_per_point();
                    let target_logical = size / 3.0; // ~1/3 of slot
                    let target_physical = target_logical * dpi_scale;
                    let physical_scale = (target_physical / sprite_native).round().max(1.0);
                    let overlay_size = (sprite_native * physical_scale) / dpi_scale;

                    let enchant_rect = Rect::from_min_size(
                        Pos2::new(rect.right() - overlay_size, rect.bottom() - overlay_size),
                        Vec2::new(overlay_size, overlay_size),
                    );
                    painter.image(
                        enchant_tex.id(),
                        enchant_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
            }

            // Draw stack count in bottom-right corner (only if stack > 1)
            if stack_count > 1 {
                let scale = size / 28.0; // 28.0 is our "standard" size
                let count_text = format!("{}", stack_count);
                let font = egui::FontId::proportional(11.0 * scale.min(1.2));
                let text_pos = egui::pos2(rect.right() - 3.0, rect.bottom() - 3.0);

                // Draw text shadow/outline for readability
                for dx in [-1.0, 0.0, 1.0] {
                    for dy in [-1.0, 0.0, 1.0] {
                        if dx != 0.0 || dy != 0.0 {
                            painter.text(
                                egui::pos2(text_pos.x + dx, text_pos.y + dy),
                                egui::Align2::RIGHT_BOTTOM,
                                &count_text,
                                font.clone(),
                                Color32::BLACK,
                            );
                        }
                    }
                }
                // Draw the actual count
                painter.text(
                    text_pos,
                    egui::Align2::RIGHT_BOTTOM,
                    &count_text,
                    font,
                    Color32::WHITE,
                );
            }

            // Draw hover highlight for clickable items
            if sense.senses_click() && response.hovered() {
                painter.rect_stroke(
                    rect,
                    4.0,
                    egui::Stroke::new(2.0_f32, Color32::from_rgb(100, 150, 200)),
                    egui::StrokeKind::Outside,
                );
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
        }

        // Tooltip - pass rect for unique ID generation
        if show_tooltip {
            // Append "(Click to filter)" hint for clickable items
            let tooltip_item_name = if sense.senses_click() {
                format!("{}\n(Click to filter)", item_name)
            } else {
                item_name.to_string()
            };
            self.render_item_tooltip(
                ui,
                rect,
                response.clone(),
                item_id,
                &tooltip_item_name,
                enchant_ids,
                is_shiny,
                stack_count,
                true,
                &[],
                "",
            );
        }

        response
    }

    /// Render an item sprite at `size` px with the enchant rarity gem overlaid,
    /// plus the full hover tooltip (item name + enchant names + stats), but with
    /// NO slot background frame and no inner padding -- the sprite fills the rect.
    /// Used by Combat History gear columns, which want bare icons like the old
    /// `draw_sprite_in_rect` look but with gems and hover info.
    ///
    /// Scales the sprite by its native frame size, so padded item art (e.g.
    /// Necromancer skulls with transparent padding) renders proportionally
    /// instead of overflowing its cell. Used by the boss-fight detail gear column.
    pub fn render_item_sprite_with_enchants_native(
        &mut self,
        ui: &mut egui::Ui,
        item_id: i32,
        enchant_ids: &[i32],
        size: f32,
    ) -> egui::Response {
        self.render_item_sprite_with_enchants_ex(ui, item_id, enchant_ids, size, true)
    }

    fn render_item_sprite_with_enchants_ex(
        &mut self,
        ui: &mut egui::Ui,
        item_id: i32,
        enchant_ids: &[i32],
        size: f32,
        native_scale: bool,
    ) -> egui::Response {
        let display_name = self
            .item_name(item_id)
            .unwrap_or_else(|| format!("Item 0x{:04X}", item_id));
        let (item_name, _stack) = Self::extract_stack_from_name(&display_name);
        let is_shiny = self.is_shiny(item_id);
        let enchant_count = enchant_ids.len();

        let slot_id = ui.next_auto_id();
        ui.push_id(slot_id, |ui| {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());

            if ui.is_rect_visible(rect) {
                let sprite_drawn = if enchant_count > 0 {
                    let glow_color = Self::enchant_glow_color(enchant_count);
                    let ctx = ui.ctx().clone();
                    self.draw_glowed_sprite(&ctx, ui, item_id, rect, glow_color, 7)
                } else if native_scale {
                    self.draw_outlined_sprite_in_rect_native(ui, item_id, rect)
                } else {
                    self.draw_outlined_sprite_in_rect(ui, item_id, rect)
                };
                if !sprite_drawn {
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        format!("{:X}", item_id),
                        egui::FontId::proportional(6.0),
                        Color32::WHITE,
                    );
                }
                if enchant_count > 0 {
                    let gem_size = size * 0.45;
                    let gem_rect = Rect::from_min_size(
                        Pos2::new(rect.right() - gem_size, rect.bottom() - gem_size),
                        Vec2::splat(gem_size),
                    );
                    let tier_idx = (enchant_count.min(4) - 1) as usize;
                    self.draw_enchant_tier_in_rect_scaled(ui, tier_idx, gem_rect);
                }
                self.draw_blueprint_unlock_overlay(ui, item_id, rect);
            }

            self.render_item_tooltip(
                ui,
                rect,
                response.clone(),
                item_id,
                &item_name,
                enchant_ids,
                is_shiny,
                1,
                true,
                &[],
                "",
            );
            response
        })
        .inner
    }

    /// Render an empty item slot (dark background only).
    pub fn render_empty_slot(&self, ui: &mut egui::Ui, size: f32) -> egui::Response {
        // Use auto-incrementing ID to ensure each slot is unique
        let slot_id = ui.next_auto_id();
        ui.push_id(slot_id, |ui| {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());

            if ui.is_rect_visible(rect) {
                let ui_visuals = ui.visuals();
                let painter = ui.painter();
                painter.rect_filled(rect, 4.0, crate::ui_colors::slot_fill(ui_visuals));
                painter.rect_stroke(
                    rect,
                    4.0,
                    egui::Stroke::new(1.0_f32, crate::ui_colors::slot_stroke(ui_visuals)),
                    egui::StrokeKind::Outside,
                );
            }

            response
        })
        .inner
    }

    /// Render an unavailable/locked item slot (darker background, dashed border).
    /// Used for expansion slots (backpack, extender) when the character doesn't have them.
    pub fn render_unavailable_slot(&self, ui: &mut egui::Ui, size: f32) -> egui::Response {
        let slot_id = ui.next_auto_id();
        ui.push_id(slot_id, |ui| {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());

            if ui.is_rect_visible(rect) {
                let ui_visuals = ui.visuals();
                let painter = ui.painter();
                // Darker background to indicate unavailable
                painter.rect_filled(
                    rect,
                    4.0,
                    crate::ui_colors::unavailable_slot_fill(ui_visuals),
                );
                // Dashed border effect using corner marks
                let stroke = egui::Stroke::new(
                    1.0_f32,
                    crate::ui_colors::unavailable_slot_stroke(ui_visuals),
                );
                let dash_len = 4.0;
                let corners = [
                    (
                        rect.left_top(),
                        egui::vec2(dash_len, 0.0),
                        egui::vec2(0.0, dash_len),
                    ),
                    (
                        rect.right_top(),
                        egui::vec2(-dash_len, 0.0),
                        egui::vec2(0.0, dash_len),
                    ),
                    (
                        rect.left_bottom(),
                        egui::vec2(dash_len, 0.0),
                        egui::vec2(0.0, -dash_len),
                    ),
                    (
                        rect.right_bottom(),
                        egui::vec2(-dash_len, 0.0),
                        egui::vec2(0.0, -dash_len),
                    ),
                ];
                for (corner, h_offset, v_offset) in corners {
                    painter.line_segment([corner, corner + h_offset], stroke);
                    painter.line_segment([corner, corner + v_offset], stroke);
                }
            }

            response
        })
        .inner
    }

    /// A momentary-turned-toggle button for the Shiny filter, sized/styled like a
    /// compact shadcn toggle but showing the in-game shiny sparkles sprite in place
    /// of a text glyph. Falls back to plain "Shiny" text if the overlay is missing.
    pub fn shiny_toggle(
        &mut self,
        ui: &mut egui::Ui,
        shadcn: &crate::shadcn_ui::Shadcn,
        on: &mut bool,
        tooltip: &str,
    ) -> egui::Response {
        self.load_overlay_sprites(ui.ctx());
        let palette = shadcn.colors();
        let (fill, stroke, text_color) = if *on {
            (
                palette.accent,
                egui::Stroke::new(1.0_f32, palette.accent),
                palette.accent_foreground,
            )
        } else {
            (
                palette.secondary,
                egui::Stroke::new(1.0_f32, palette.border),
                palette.secondary_foreground,
            )
        };
        let label = egui::RichText::new("Shiny").small().color(text_color);
        let resp = ui
            .scope(|ui| {
                ui.spacing_mut().button_padding = egui::vec2(6.0, 2.0);
                let btn = if let Some(tex) = &self.shiny_texture {
                    let img = egui::Image::new(egui::load::SizedTexture::new(
                        tex.id(),
                        egui::vec2(14.0, 14.0),
                    ));
                    egui::Button::image_and_text(img, label)
                } else {
                    egui::Button::new(label)
                };
                ui.add(btn.fill(fill).stroke(stroke).corner_radius(4.0))
            })
            .inner
            .hover_tip(tooltip);
        if resp.clicked() {
            *on = !*on;
        }
        resp
    }

    /// `rarities` is a list of (enchant slot-count, rarity name, whether that rarity has
    /// been discovered) tuples, in display order. An item may show as discovered even if
    /// no longer present on the account (e.g. lost on a dead character, traded, or fed).
    pub fn show_item_tooltip_with_rarities(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        item_id: i32,
        item_name: &str,
        rarities: &[(u8, &str, bool)],
    ) {
        let is_shiny = self.is_shiny(item_id);
        let rect = response.rect;
        let rarities: Vec<(u8, &str, bool, Option<u32>)> = rarities
            .iter()
            .map(|&(slots, name, discovered)| (slots, name, discovered, None))
            .collect();
        self.render_item_tooltip(
            ui,
            rect,
            response.clone(),
            item_id,
            item_name,
            &[],
            is_shiny,
            1,
            true,
            &rarities,
            "Rarities discovered",
        );
    }

    /// Render an item tile (sprite + tooltip) with a "Rarities tracked" section appended,
    /// showing the tracked drop count per enchant slot-count. Used by the Trophy Hall
    /// tracked-loot section so the rarity breakdown lives in the same tooltip as the
    /// rest of the item info, instead of a separate overlapping popup.
    pub fn render_item_tile_with_rarity_counts(
        &mut self,
        ui: &mut egui::Ui,
        item_id: i32,
        raw_name: Option<&str>,
        rarity_counts: &[(u8, &str, u32)],
    ) -> egui::Response {
        const TILE_SIZE: f32 = 38.0;

        let display_name = match raw_name {
            Some(name)
                if !name.starts_with("Unknown (")
                    && !(name.starts_with('{') && name.ends_with('}')) =>
            {
                name.to_string()
            }
            _ => self.item_name(item_id).unwrap_or_else(|| {
                raw_name
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| format!("Item 0x{:04X}", item_id))
            }),
        };
        let (item_name, stack_count) = Self::extract_stack_from_name(&display_name);
        let is_shiny = self.is_shiny(item_id);

        let slot_id = ui.next_auto_id();
        let response = ui
            .push_id(slot_id, |ui| {
                self.render_item_slot_inner_no_tooltip(
                    ui,
                    item_id,
                    &item_name,
                    &[],
                    is_shiny,
                    TILE_SIZE,
                    stack_count,
                    egui::Sense::hover(),
                )
            })
            .inner;

        let rarities: Vec<(u8, &str, bool, Option<u32>)> = rarity_counts
            .iter()
            .map(|&(slots, name, count)| (slots, name, true, Some(count)))
            .collect();
        let rect = response.rect;
        self.render_item_tooltip(
            ui,
            rect,
            response.clone(),
            item_id,
            &item_name,
            &[],
            is_shiny,
            stack_count,
            true,
            &rarities,
            "Rarities tracked",
        );

        response
    }

    /// Render item tooltip with name, shiny indicator, enchantments, item stats, and ID.
    /// When `show_sprite` is true, renders the item tile (with outline) below the name.
    /// `rarities` optionally appends a section titled `rarities_label` (slot-count, name,
    /// discovered/tracked, optional tracked count).
    fn render_item_tooltip(
        &mut self,
        _ui: &egui::Ui,
        rect: egui::Rect,
        response: egui::Response,
        item_id: i32,
        item_name: &str,
        enchant_ids: &[i32],
        is_shiny: bool,
        stack_count: u8,
        show_sprite: bool,
        rarities: &[(u8, &str, bool, Option<u32>)],
        rarities_label: &str,
    ) {
        use eframe::egui::RichText;
        use realmhound_core::assets::get_asset_manager;

        response.clone().hover_tip_ui(|ui| {
            // Set max width explicitly to prevent overly wide tooltips
            ui.set_max_width(200.0);

            // Item name
            ui.label(RichText::new(item_name).strong());

            if show_sprite {
                self.render_item_slot_inner_no_tooltip(
                    ui,
                    item_id,
                    item_name,
                    enchant_ids,
                    is_shiny,
                    38.0,
                    stack_count,
                    egui::Sense::hover(),
                );
            }

            // Blueprint: show the item it unlocks and the forge (dungeon)
            // collection it counts toward, both with inline sprites.
            if let Some(obj) = get_asset_manager().get_object(item_id) {
                if obj.is_blueprint() {
                    let asset_mgr = get_asset_manager();
                    let sprite_size = 14.0;
                    let text_col = Color32::from_rgb(180, 180, 220);
                    let unlocked = asset_mgr.blueprint_unlocked_item(item_id);
                    let dungeon = obj.collection_icon.and_then(|icon| {
                        asset_mgr
                            .dungeon_name_for_collection_icon(icon)
                            .map(|name| (icon, name))
                    });
                    if unlocked.is_some() || dungeon.is_some() {
                        ui.separator();
                    }
                    if let Some((unlocked_id, unlocked_name)) = unlocked {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            ui.label(RichText::new("Unlocks").small().color(text_col));
                            if let Some(s) = asset_mgr.get_object_sprite(unlocked_id) {
                                self.draw_sprite_from_atlas(ui, &s, sprite_size);
                            }
                            ui.label(
                                RichText::new(unlocked_name)
                                    .small()
                                    .strong()
                                    .color(text_col),
                            );
                        });
                    }
                    if let Some((icon, dungeon)) = dungeon {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            ui.label(
                                RichText::new("Counts in the forge as item from")
                                    .small()
                                    .color(text_col),
                            );
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(sprite_size, sprite_size),
                                egui::Sense::hover(),
                            );
                            self.draw_collection_icon(ui, icon, rect);
                            ui.label(
                                RichText::new(format!("{dungeon} collection"))
                                    .small()
                                    .color(text_col),
                            );
                        });
                    }
                }
            }

            if is_shiny {
                ui.label(
                    RichText::new("⭐ Shiny")
                        .small()
                        .color(Color32::from_rgb(255, 215, 0)),
                );
            }

            // Show enchantments with names and sprites
            if !enchant_ids.is_empty() {
                ui.separator();

                for &ench_id in enchant_ids {
                    // Negative sentinels mark unfilled slots: -1 empty, -2 locked.
                    if ench_id < 0 {
                        let (label, color) = if ench_id == -2 {
                            ("Locked", Color32::from_rgb(180, 120, 120))
                        } else {
                            ("<empty>", Color32::from_gray(150))
                        };
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("  •").small());
                            ui.label(RichText::new(label).small().italics().color(color));
                        });
                        continue;
                    }

                    let name = self
                        .enchant_name(ench_id as u16)
                        .unwrap_or_else(|| format!("Unknown (0x{:03X})", ench_id));

                    ui.horizontal(|ui| {
                        // Try to render enchant sprite (16x16 scaled to 14)
                        let sprite_size = 14.0;
                        let sprite_rendered =
                            self.draw_enchant_sprite_in_tooltip(ui, ench_id as u16, sprite_size);

                        if !sprite_rendered {
                            // Fallback: just show bullet
                            ui.label(RichText::new("  •").small());
                        }

                        ui.label(RichText::new(&name).small());
                    });
                }
            }

            // Stack count (only show if > 1)
            if stack_count > 1 {
                ui.label(
                    RichText::new(format!("Stack: {}", stack_count))
                        .small()
                        .weak(),
                );
            }

            // Show item stats (tier, feed power, fame bonus) if available
            if let Some(obj) = get_asset_manager().get_object(item_id) {
                let has_stats = obj.tier.is_some() || obj.feed_power > 0 || obj.fame_bonus > 0;
                if has_stats {
                    ui.separator();

                    // Tier (show "T0" through "T14", or "UT"/"ST" based on labels)
                    if let Some(tier) = obj.tier {
                        ui.label(RichText::new(format!("Tier: T{}", tier)).small());
                    } else if obj.labels.contains("UT") {
                        ui.label(
                            RichText::new("Tier: UT")
                                .small()
                                .color(Color32::from_rgb(138, 43, 226)),
                        ); // Purple for UT
                    } else if obj.labels.contains("ST") {
                        ui.label(
                            RichText::new("Tier: ST")
                                .small()
                                .color(Color32::from_rgb(255, 165, 0)),
                        ); // Orange for ST
                    }

                    // Feed Power
                    if obj.feed_power > 0 {
                        ui.label(
                            RichText::new(format!("Feed Power: {}", obj.feed_power))
                                .small()
                                .color(Color32::from_rgb(165, 214, 167)),
                        ); // Light green
                    }

                    // Fame Bonus
                    if obj.fame_bonus > 0 {
                        ui.label(
                            RichText::new(format!("Fame Bonus: {}%", obj.fame_bonus))
                                .small()
                                .color(Color32::from_rgb(255, 215, 0)),
                        ); // Gold
                    }
                }
            }

            // Dropped by section (from RealmEye data)
            let realmeye = realmhound_core::assets::get_realmeye_drops();
            if realmhound_core::assets::is_legendary_fishing_rod(item_id) {
                // Tinkerer-only reward: show how it is obtained instead of a drop
                // source, with inline sprites for the Fishing Award and crate.
                ui.separator();
                ui.label(RichText::new("Obtained by:").small().strong());
                let asset_mgr = get_asset_manager();
                let sprite_size = 14.0;
                let text_col = Color32::from_rgb(180, 180, 220);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    ui.label(RichText::new("Trade").small().color(text_col));
                    if let Some(s) =
                        asset_mgr.get_object_sprite(realmhound_core::assets::FISHING_AWARD_ITEM_ID)
                    {
                        self.draw_sprite_from_atlas(ui, &s, sprite_size);
                    }
                    ui.label(
                        RichText::new("Fishing Award (drops from")
                            .small()
                            .color(text_col),
                    );
                    if let Some(s) = asset_mgr
                        .get_object_sprite(realmhound_core::assets::MV_FISHING_LOOT_OBJECT_ID)
                    {
                        self.draw_sprite_from_atlas(ui, &s, sprite_size);
                    }
                    ui.label(
                        RichText::new("MV Fishing Loot) at the Tinkerer.")
                            .small()
                            .color(text_col),
                    );
                });
            } else if realmhound_core::assets::is_plagued_nest_beehemoth_quiver(item_id) {
                // No RealmEye data for the Green Beehemoth Quiver; show a custom
                // note pairing the Killer Bee Queen with the Plagued Nest portal
                // to mark the dungeon-specific version.
                ui.separator();
                ui.label(RichText::new("Drops from:").small().strong());
                let asset_mgr = get_asset_manager();
                let sprite_size = 14.0;
                let text_col = Color32::from_rgb(180, 180, 220);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    if let Some(s) = asset_mgr
                        .get_object_sprite(realmhound_core::assets::KILLER_BEE_QUEEN_OBJECT_ID)
                    {
                        self.draw_sprite_from_atlas(ui, &s, sprite_size);
                    }
                    ui.label(RichText::new("Killer Bee Queen (").small().color(text_col));
                    if let Some(s) = asset_mgr
                        .get_object_sprite(realmhound_core::assets::PLAGUED_NEST_PORTAL_OBJECT_ID)
                    {
                        self.draw_sprite_from_atlas(ui, &s, sprite_size);
                    }
                    ui.label(
                        RichText::new("Plagued Nest version)")
                            .small()
                            .color(text_col),
                    );
                });
            } else if realmhound_core::assets::is_forge_crafted(item_id) {
                ui.separator();
                ui.label(RichText::new("Drops from:").small().strong());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("•").small());
                    ui.label(
                        RichText::new("Forge craft")
                            .small()
                            .color(Color32::from_rgb(180, 180, 220)),
                    );
                });
            } else if let Some(source_name) = realmhound_core::assets::source_name_override(item_id)
            {
                ui.separator();
                ui.label(RichText::new("Drops from:").small().strong());
                ui.horizontal(|ui| {
                    let asset_mgr = get_asset_manager();
                    let sprite_size = 14.0;
                    let mut sprite_drawn = false;
                    if let Some(mob_obj_id) = asset_mgr.killer_sprite_id(source_name) {
                        let mob_obj_id = realmhound_core::assets::normalize_train_sprite(
                            mob_obj_id,
                            source_name,
                        );
                        if let Some(sprite_data) = asset_mgr.get_object_sprite(mob_obj_id) {
                            sprite_drawn =
                                self.draw_sprite_from_atlas(ui, &sprite_data, sprite_size);
                        }
                    }
                    if !sprite_drawn {
                        ui.label(RichText::new("•").small());
                    }
                    ui.label(
                        RichText::new(source_name)
                            .small()
                            .color(Color32::from_rgb(180, 180, 220)),
                    );
                });
            } else {
                // Shiny variants added before RealmEye's scrape lists them carry no
                // drop data of their own; fall back to the non-shiny counterpart
                // (which shares the same in-game display name) so the shiny inherits
                // its "Drops from" sources.
                let asset_mgr = get_asset_manager();
                let mut lookup_id = item_id;
                if realmeye.sources_for_item(item_id).is_empty() && asset_mgr.is_shiny(item_id) {
                    if let Some(base) = asset_mgr
                        .ids_sharing_display_name(item_id)
                        .into_iter()
                        .find(|&id| {
                            id != item_id && !asset_mgr.is_shiny(id) && realmeye.has_item(id)
                        })
                    {
                        lookup_id = base;
                    }
                }
                let sources = realmeye.sources_for_item(lookup_id);
                if !sources.is_empty() {
                    ui.separator();
                    ui.label(RichText::new("Drops from:").small().strong());

                    let mut shown: Vec<&str> = Vec::new();
                    for source in sources {
                        if shown.contains(&source.source_name.as_str()) {
                            continue;
                        }
                        // Cap at 8 so the alien UTs can show all their sources
                        // (up to 4 original + 4 Neo bosses); crowded items still
                        // truncate with a "+N more" summary.
                        if shown.len() >= 8 {
                            let remaining = sources
                                .iter()
                                .map(|s| s.source_name.as_str())
                                .collect::<std::collections::HashSet<_>>()
                                .len()
                                - shown.len();
                            if remaining > 0 {
                                ui.label(
                                    RichText::new(format!("  +{} more", remaining))
                                        .small()
                                        .weak(),
                                );
                            }
                            break;
                        }
                        shown.push(&source.source_name);

                        ui.horizontal(|ui| {
                            let sprite_size = 14.0;
                            let mut sprite_drawn = false;

                            if RealmEyeDropData::is_category_source(&source.source_name) {
                                // Category source - show location name
                                ui.label(RichText::new("•").small());
                                ui.label(
                                    RichText::new(format!(
                                        "{} of {}",
                                        &source.source_name, &source.location_name
                                    ))
                                    .small()
                                    .color(Color32::from_rgb(180, 180, 220)),
                                );
                            } else {
                                if let Some(mob_obj_id) =
                                    asset_mgr.killer_sprite_id(&source.source_name)
                                {
                                    let mob_obj_id =
                                        realmhound_core::assets::normalize_train_sprite(
                                            mob_obj_id,
                                            &source.source_name,
                                        );
                                    if let Some(sprite_data) =
                                        asset_mgr.get_object_sprite(mob_obj_id)
                                    {
                                        sprite_drawn = self.draw_sprite_from_atlas(
                                            ui,
                                            &sprite_data,
                                            sprite_size,
                                        );
                                    }
                                }

                                if !sprite_drawn {
                                    ui.label(RichText::new("•").small());
                                }

                                ui.label(
                                    RichText::new(&source.source_name)
                                        .small()
                                        .color(Color32::from_rgb(180, 180, 220)),
                                );
                            }
                        });
                    }
                }
            }

            // Soulful Affection also has a combine recipe (Gem of Tenderness +
            // Gem of Adoration), shown as an extra note alongside its drop source.
            if realmhound_core::assets::is_soulful_affection(item_id) {
                let asset_mgr = get_asset_manager();
                let sprite_size = 14.0;
                let text_col = Color32::from_rgb(180, 180, 220);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    ui.label(
                        RichText::new("Also obtainable by combining the")
                            .small()
                            .color(text_col),
                    );
                    if let Some(s) = asset_mgr
                        .get_object_sprite(realmhound_core::assets::GEM_OF_TENDERNESS_ITEM_ID)
                    {
                        self.draw_sprite_from_atlas(ui, &s, sprite_size);
                    }
                    ui.label(
                        RichText::new("Gem of Tenderness and")
                            .small()
                            .color(text_col),
                    );
                    if let Some(s) = asset_mgr
                        .get_object_sprite(realmhound_core::assets::GEM_OF_ADORATION_ITEM_ID)
                    {
                        self.draw_sprite_from_atlas(ui, &s, sprite_size);
                    }
                    ui.label(RichText::new("Gem of Adoration.").small().color(text_col));
                });
            }

            // Rarities discovered/tracked (Trophy Hall collection & tracked-loot views)
            if !rarities.is_empty() {
                ui.separator();
                ui.label(RichText::new(rarities_label).small().strong());
                for &(slots, name, obtained, count) in rarities {
                    ui.horizontal(|ui| {
                        let (icon_rect, _) =
                            ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::hover());
                        if obtained {
                            self.draw_outlined_sprite_in_rect(ui, item_id, icon_rect);
                        } else {
                            self.draw_outlined_sprite_in_rect_tinted(
                                ui,
                                item_id,
                                icon_rect,
                                Color32::from_rgb(30, 30, 30),
                            );
                        }
                        if slots > 0 {
                            let gem_size = 9.0;
                            let gem_rect = Rect::from_min_size(
                                icon_rect.right_bottom() - egui::vec2(gem_size, gem_size),
                                egui::vec2(gem_size, gem_size),
                            );
                            let gem_tint = if obtained {
                                Color32::WHITE
                            } else {
                                Color32::from_rgb(30, 30, 30)
                            };
                            self.draw_enchant_tier_in_rect_tinted(
                                ui,
                                (slots - 1) as usize,
                                gem_rect,
                                gem_tint,
                            );
                        }
                        let color = if obtained {
                            Color32::WHITE
                        } else {
                            Color32::from_gray(90)
                        };
                        let label_text = match count {
                            Some(n) => format!("{} ×{}", name, n),
                            None => name.to_string(),
                        };
                        ui.label(RichText::new(label_text).small().color(color));
                    });
                }
            } else if let Some(counts) = self
                .owned_rarities
                .get(&item_id)
                .copied()
                .filter(|_| !self.hide_owned_rarity)
            {
                // Owned-rarity breakdown for enchantable equipment the account
                // owns (Treasury / Characters / Vault and generic tooltips).
                let is_gear = crate::rendering::rarity::is_enchantable(item_id);
                let possible = crate::rendering::rarity::possible_rarities(item_id);
                let total: u32 = possible.iter().map(|&s| counts[s as usize]).sum();
                if is_gear && !possible.is_empty() && total > 0 {
                    ui.separator();
                    ui.label(RichText::new("Owned rarity").small().strong());
                    for &slots in possible {
                        let n = counts[slots as usize];
                        let obtained = n > 0;
                        let name = crate::rendering::rarity::rarity_name(slots);
                        ui.horizontal(|ui| {
                            let (icon_rect, _) = ui
                                .allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::hover());
                            if obtained {
                                self.draw_outlined_sprite_in_rect(ui, item_id, icon_rect);
                            } else {
                                self.draw_outlined_sprite_in_rect_tinted(
                                    ui,
                                    item_id,
                                    icon_rect,
                                    Color32::from_rgb(30, 30, 30),
                                );
                            }
                            if slots > 0 {
                                let gem_size = 9.0;
                                let gem_rect = Rect::from_min_size(
                                    icon_rect.right_bottom() - egui::vec2(gem_size, gem_size),
                                    egui::vec2(gem_size, gem_size),
                                );
                                let gem_tint = if obtained {
                                    Color32::WHITE
                                } else {
                                    Color32::from_rgb(30, 30, 30)
                                };
                                self.draw_enchant_tier_in_rect_tinted(
                                    ui,
                                    (slots - 1) as usize,
                                    gem_rect,
                                    gem_tint,
                                );
                            }
                            let color = if obtained {
                                Color32::WHITE
                            } else {
                                Color32::from_gray(90)
                            };
                            ui.label(
                                RichText::new(format!("{} ×{}", name, n))
                                    .small()
                                    .color(color),
                            );
                        });
                    }
                }
            }

            ui.separator();
            ui.label(
                RichText::new(format!("ID: 0x{:04X}", item_id))
                    .small()
                    .weak(),
            );
        });

        // Suppress the unused variable warning for rect/tooltip_id
        let _ = rect;
    }

    /// Draw an enchantment sprite in a tooltip context.
    /// Returns true if the sprite was rendered successfully.
    fn draw_enchant_sprite_in_tooltip(
        &self,
        ui: &mut egui::Ui,
        enchant_id: u16,
        size: f32,
    ) -> bool {
        let sprite_data = match get_asset_manager().get_enchant_sprite(enchant_id) {
            Some(s) => s,
            None => return false,
        };

        let texture = match self.textures.get(&sprite_data.atlas_id) {
            Some(t) => t,
            None => return false,
        };

        let (atlas_w, atlas_h) = match self.atlas_dimensions.get(&sprite_data.atlas_id) {
            Some(&dims) => dims,
            None => return false,
        };
        if Self::checked_region(
            sprite_data.x,
            sprite_data.y,
            sprite_data.width,
            sprite_data.height,
            atlas_w,
            atlas_h,
        )
        .is_none()
        {
            return false;
        }

        // Calculate UV coordinates
        let uv_min = Pos2::new(
            sprite_data.x as f32 / atlas_w as f32,
            sprite_data.y as f32 / atlas_h as f32,
        );
        let uv_max = Pos2::new(
            (sprite_data.x + sprite_data.width) as f32 / atlas_w as f32,
            (sprite_data.y + sprite_data.height) as f32 / atlas_h as f32,
        );

        // Allocate space and draw
        let (rect, _response) = ui.allocate_exact_size(Vec2::new(size, size), egui::Sense::hover());

        if ui.is_rect_visible(rect) {
            let uv = Rect::from_min_max(uv_min, uv_max);
            ui.painter().image(texture.id(), rect, uv, Color32::WHITE);
        }

        true
    }

    /// Draw an enchantment sprite (16x16) centered in `rect`. Public variant of
    /// [`Self::draw_enchant_sprite_in_tooltip`] for settings rows and pickers.
    /// Returns false if the sprite or its atlas is not loaded.
    pub fn draw_enchant_sprite_in_rect(&self, ui: &egui::Ui, enchant_id: u16, rect: Rect) -> bool {
        if !ui.is_rect_visible(rect) {
            return true;
        }
        let sprite_data = match get_asset_manager().get_enchant_sprite(enchant_id) {
            Some(s) => s,
            None => return false,
        };
        let texture = match self.textures.get(&sprite_data.atlas_id) {
            Some(t) => t,
            None => return false,
        };
        let (atlas_w, atlas_h) = match self.atlas_dimensions.get(&sprite_data.atlas_id) {
            Some(&dims) => dims,
            None => return false,
        };
        if Self::checked_region(
            sprite_data.x,
            sprite_data.y,
            sprite_data.width,
            sprite_data.height,
            atlas_w,
            atlas_h,
        )
        .is_none()
        {
            return false;
        }
        let uv_min = Pos2::new(
            sprite_data.x as f32 / atlas_w as f32,
            sprite_data.y as f32 / atlas_h as f32,
        );
        let uv_max = Pos2::new(
            (sprite_data.x + sprite_data.width) as f32 / atlas_w as f32,
            (sprite_data.y + sprite_data.height) as f32 / atlas_h as f32,
        );
        let uv = Rect::from_min_max(uv_min, uv_max);
        ui.painter().image(texture.id(), rect, uv, Color32::WHITE);
        true
    }

    /// Draw any sprite from atlas data at a given size. Read-only (no caching).
    fn draw_sprite_from_atlas(
        &mut self,
        ui: &mut egui::Ui,
        sprite_data: &realmhound_core::assets::SpriteData,
        size: f32,
    ) -> bool {
        // Trim transparent padding and preserve aspect so padded sheets (e.g.
        // 32x32 boss sprites) render tight and aligned with the adjacent text.
        let Some((tx, ty, tw, th)) = self.trimmed_region(
            sprite_data.atlas_id,
            sprite_data.x,
            sprite_data.y,
            sprite_data.width,
            sprite_data.height,
        ) else {
            return false;
        };

        let texture = match self.textures.get(&sprite_data.atlas_id) {
            Some(t) => t,
            None => return false,
        };

        let (atlas_w, atlas_h) = match self.atlas_dimensions.get(&sprite_data.atlas_id) {
            Some(&dims) => dims,
            None => return false,
        };

        let uv_min = Pos2::new(tx as f32 / atlas_w as f32, ty as f32 / atlas_h as f32);
        let uv_max = Pos2::new(
            (tx + tw) as f32 / atlas_w as f32,
            (ty + th) as f32 / atlas_h as f32,
        );

        let (w, h) = (tw.max(1) as f32, th.max(1) as f32);
        let fit = size / w.max(h);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w * fit, h * fit), egui::Sense::hover());

        if ui.is_rect_visible(rect) {
            let uv = Rect::from_min_max(uv_min, uv_max);
            ui.painter().image(texture.id(), rect, uv, Color32::WHITE);
        }

        true
    }

    /// Render a filter pill (icon + label + close button).
    /// Shared widget used by Treasury and Loot panels.
    /// Returns true if the close button was clicked.
    pub fn render_filter_pill(
        &mut self,
        ui: &mut egui::Ui,
        icon_id: i32,
        label: &str,
        bg_color: Color32,
    ) -> bool {
        const H: f32 = 20.0;
        const LEFT_PAD: f32 = 4.0;
        const ICON: f32 = 16.0;
        const GAP1: f32 = 4.0; // icon -> text
        const GAP2: f32 = 6.0; // text -> ✕
        const X_W: f32 = 10.0;
        const RIGHT_PAD: f32 = 6.0;

        // A single fixed-width widget (icon + label + ✕), not a nested
        // `Frame`/`horizontal`: the enclosing `horizontal_wrapped` filter bar can
        // only measure a pill's width -- and thus wrap it onto a second row
        // instead of letting it overflow the right edge -- when the pill is one
        // atomic allocation. Clicking anywhere on the pill removes its filter.
        let font = egui::FontId::proportional(11.0);
        let galley = ui
            .painter()
            .layout_no_wrap(label.to_string(), font.clone(), Color32::WHITE);
        let text_w = galley.size().x;
        let width = LEFT_PAD + ICON + GAP1 + text_w + GAP2 + X_W + RIGHT_PAD;

        let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, H), egui::Sense::click());
        let hovered = resp.hovered();
        let clicked = resp.clicked();
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(4), bg_color);

        let icon_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left() + LEFT_PAD, rect.center().y - ICON / 2.0),
            egui::vec2(ICON, ICON),
        );
        if icon_id > 0 {
            if !self.draw_outlined_sprite_in_rect_filled(ui, icon_id, icon_rect) {
                self.draw_sprite_in_rect(ui, icon_id, icon_rect);
            }
        } else {
            ui.painter().text(
                icon_rect.center(),
                egui::Align2::CENTER_CENTER,
                "?",
                egui::FontId::proportional(10.0),
                Color32::WHITE,
            );
        }

        let text_pos = egui::pos2(
            icon_rect.right() + GAP1,
            rect.center().y - galley.size().y / 2.0,
        );
        ui.painter().galley(text_pos, galley, Color32::WHITE);

        let x_center = egui::pos2(rect.right() - RIGHT_PAD - X_W / 2.0, rect.center().y);
        let x_color = if hovered {
            Color32::from_rgb(255, 120, 120)
        } else {
            Color32::from_gray(200)
        };
        ui.painter().text(
            x_center,
            egui::Align2::CENTER_CENTER,
            "\u{2715}",
            font,
            x_color,
        );

        resp.on_hover_text(format!("Remove {label} filter"));
        clicked
    }

    /// Filter pill that renders a dyed, outlined character sprite as its icon
    /// (matching how player rows are drawn in the fight roster). Falls back to a
    /// flat sprite, then a "?" glyph, when dye/sprite data is unavailable.
    pub fn render_filter_pill_character(
        &mut self,
        ui: &mut egui::Ui,
        sprite_id: i32,
        tex1: u32,
        tex2: u32,
        label: &str,
        bg_color: Color32,
    ) -> bool {
        const H: f32 = 20.0;
        const LEFT_PAD: f32 = 4.0;
        const ICON: f32 = 16.0;
        const GAP1: f32 = 4.0;
        const GAP2: f32 = 6.0;
        const X_W: f32 = 10.0;
        const RIGHT_PAD: f32 = 6.0;

        // Single atomic widget (see [`Self::render_filter_pill`]) so a wrapping
        // filter bar can measure and fold it. Clicking removes the filter.
        let font = egui::FontId::proportional(11.0);
        let galley = ui
            .painter()
            .layout_no_wrap(label.to_string(), font.clone(), Color32::WHITE);
        let text_w = galley.size().x;
        let width = LEFT_PAD + ICON + GAP1 + text_w + GAP2 + X_W + RIGHT_PAD;

        let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, H), egui::Sense::click());
        let hovered = resp.hovered();
        let clicked = resp.clicked();
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(4), bg_color);

        let icon_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left() + LEFT_PAD, rect.center().y - ICON / 2.0),
            egui::vec2(ICON, ICON),
        );
        let drawn = sprite_id > 0
            && (self.draw_dyed_outlined_character_sprite(ui, sprite_id, icon_rect, 6, tex1, tex2)
                || self.draw_sprite_in_rect(ui, sprite_id, icon_rect));
        if !drawn {
            ui.painter().text(
                icon_rect.center(),
                egui::Align2::CENTER_CENTER,
                "?",
                egui::FontId::proportional(10.0),
                Color32::WHITE,
            );
        }

        let text_pos = egui::pos2(
            icon_rect.right() + GAP1,
            rect.center().y - galley.size().y / 2.0,
        );
        ui.painter().galley(text_pos, galley, Color32::WHITE);

        let x_center = egui::pos2(rect.right() - RIGHT_PAD - X_W / 2.0, rect.center().y);
        let x_color = if hovered {
            Color32::from_rgb(255, 120, 120)
        } else {
            Color32::from_gray(200)
        };
        ui.painter().text(
            x_center,
            egui::Align2::CENTER_CENTER,
            "\u{2715}",
            font,
            x_color,
        );

        resp.on_hover_text(format!("Remove {label} filter"));
        clicked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sprite_renderer_new() {
        let renderer = SpriteRenderer::new();
        assert!(renderer.textures.is_empty());
    }

    #[test]
    fn checked_region_accepts_edge_touching_region() {
        assert_eq!(
            SpriteRenderer::checked_region(8184, 4088, 8, 8, 8192, 4096),
            Some((8184, 4088, 8, 8))
        );
    }

    #[test]
    fn checked_region_rejects_lower_edge_overflow() {
        assert_eq!(
            SpriteRenderer::checked_region(1444, 4089, 8, 8, 8192, 4096),
            None
        );
    }

    #[test]
    fn checked_region_rejects_negative_coordinates() {
        assert_eq!(
            SpriteRenderer::checked_region(-1, 0, 8, 8, 8192, 4096),
            None
        );
        assert_eq!(
            SpriteRenderer::checked_region(0, -1, 8, 8, 8192, 4096),
            None
        );
    }

    #[test]
    fn checked_region_rejects_invalid_size() {
        assert_eq!(SpriteRenderer::checked_region(0, 0, 0, 8, 8192, 4096), None);
        assert_eq!(
            SpriteRenderer::checked_region(0, 0, 8, -1, 8192, 4096),
            None
        );
        assert_eq!(
            SpriteRenderer::checked_region(8190, 0, i32::MAX, 8, 8192, 4096),
            None
        );
    }
}

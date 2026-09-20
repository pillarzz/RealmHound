//! Sprite atlas handling for RotMG game sprites.
//!
//! This module handles loading sprite coordinates from the FlatBuffer format
//! used by RotMG, and loading sprite atlas PNG images.
//!
//! # Architecture
//!
//! The sprite system uses a two-level lookup:
//! 1. FlatBuffer file contains sprite coordinates indexed by (sheet_name, sprite_index)
//! 2. PNG atlas images contain the actual pixel data
//!
//! When rendering, we look up the sprite coordinates, then crop the appropriate
//! region from the atlas image.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Sprite coordinate data from the FlatBuffer file.
#[derive(Debug, Clone, Copy)]
pub struct SpriteData {
    /// X coordinate in atlas
    pub x: i32,
    /// Y coordinate in atlas  
    pub y: i32,
    /// Width of sprite
    pub width: i32,
    /// Height of sprite
    pub height: i32,
    /// Atlas ID (1-4)
    pub atlas_id: u8,
    /// Most common color (RGBA)
    pub color: [u8; 4],
    /// Mask position in the characters_masks atlas (for dye/cloth rendering).
    /// Present for character sprites that support tex1/tex2 compositing.
    pub mask: Option<MaskPosition>,
}

/// Mask sprite position within the characters_masks atlas.
#[derive(Debug, Clone, Copy)]
pub struct MaskPosition {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl SpriteData {
    /// Get the sprite region as (x, y, width, height).
    pub fn region(&self) -> (i32, i32, i32, i32) {
        (self.x, self.y, self.width, self.height)
    }
}

/// Key for animated sprite lookup: (sheet_name, index, direction)
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct AnimatedSpriteKey {
    name: String,
    index: i32,
    direction: i32,
}

/// Sprite sheet containing multiple sprites indexed by their position.
#[derive(Debug, Default)]
struct SpriteSheet {
    /// Map from sprite index to sprite data
    sprites: HashMap<i32, SpriteData>,
}

/// Sprite atlas manager for loading and caching sprite data.
///
/// This handles the FlatBuffer sprite coordinate data. The actual PNG
/// loading is done separately by the AssetManager for egui texture handling.
#[derive(Debug, Default)]
pub struct SpriteAtlas {
    /// Map from sheet name to sprite sheet
    sheets: HashMap<String, SpriteSheet>,
    /// Map for animated sprites with direction: (name, index, direction) -> SpriteData
    animated_sprites: HashMap<AnimatedSpriteKey, SpriteData>,
    /// Whether data has been loaded
    loaded: bool,
}

impl SpriteAtlas {
    /// Create a new empty sprite atlas.
    pub fn new() -> Self {
        Self {
            sheets: HashMap::new(),
            animated_sprites: HashMap::new(),
            loaded: false,
        }
    }

    /// Load sprite data from the FlatBuffer file.
    ///
    /// The FlatBuffer format is complex, so we use a simplified parser
    /// that reads the binary data directly based on the known structure.
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, SpriteAtlasError> {
        let mut file = File::open(path)?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;

        Self::parse_flatbuffer(&data)
    }

    /// Parse the FlatBuffer data.
    ///
    /// The FlatBuffer format for spritesheetf:
    /// - Root table with vector of SpriteSheet tables
    /// - Each SpriteSheet has name (string) and sprites (vector of Sprite tables)
    /// - Each Sprite has: position (struct), index (int), mostCommonColor (struct), aId (long)
    fn parse_flatbuffer(data: &[u8]) -> Result<Self, SpriteAtlasError> {
        if data.len() < 8 {
            return Err(SpriteAtlasError::InvalidFormat(
                "File too small".to_string(),
            ));
        }

        // FlatBuffer root offset is at position 0 (4 bytes, little-endian)
        let root_offset = read_u32(data, 0) as usize;
        let root_pos = root_offset;

        // Read vtable offset (negative offset from root position)
        let vtable_offset = read_i32(data, root_pos) as i32;
        let vtable_pos = (root_pos as i32 - vtable_offset) as usize;

        // VTable: [vtable_size: u16, object_size: u16, field_offsets...]
        let vtable_size = read_u16(data, vtable_pos) as usize;

        // Calculate number of fields from vtable size
        // vtable has: vtable_size (2) + object_size (2) + field_offsets (2 each)
        let num_fields = (vtable_size - 4) / 2;

        if num_fields < 1 {
            return Err(SpriteAtlasError::InvalidFormat(
                "No sprite sheets field".to_string(),
            ));
        }

        // Field 0: sprites vector offset
        let sprites_field_offset = read_u16(data, vtable_pos + 4) as usize;
        if sprites_field_offset == 0 {
            return Ok(Self::new()); // No sprites
        }

        // Get the actual vector offset
        let sprites_offset_pos = root_pos + sprites_field_offset;
        let sprites_vector_offset = read_u32(data, sprites_offset_pos) as usize;
        let sprites_vector_pos = sprites_offset_pos + sprites_vector_offset;

        // Vector format: [length: u32, elements...]
        let num_sheets = read_u32(data, sprites_vector_pos) as usize;

        let mut atlas = SpriteAtlas::new();

        // Parse each sprite sheet
        for i in 0..num_sheets {
            // Each element is an offset to a table
            let sheet_offset_pos = sprites_vector_pos + 4 + (i * 4);
            let sheet_offset = read_u32(data, sheet_offset_pos) as usize;
            let sheet_pos = sheet_offset_pos + sheet_offset;

            if let Ok((name, sprites)) = Self::parse_sprite_sheet(data, sheet_pos) {
                let mut sheet = SpriteSheet::default();
                sheet.sprites = sprites;
                atlas.sheets.insert(name, sheet);
            }
        }

        // Also parse animated sprites if present (field 1)
        if num_fields >= 2 {
            let animated_field_offset = read_u16(data, vtable_pos + 6) as usize;
            if animated_field_offset != 0 {
                let animated_offset_pos = root_pos + animated_field_offset;
                let animated_vector_offset = read_u32(data, animated_offset_pos) as usize;
                let animated_vector_pos = animated_offset_pos + animated_vector_offset;
                let num_animated = read_u32(data, animated_vector_pos) as usize;

                for i in 0..num_animated {
                    let anim_offset_pos = animated_vector_pos + 4 + (i * 4);
                    let anim_offset = read_u32(data, anim_offset_pos) as usize;
                    let anim_pos = anim_offset_pos + anim_offset;

                    if let Ok((name, index, direction, action, sprite)) =
                        Self::parse_animated_sprite(data, anim_pos)
                    {
                        // Store in animated sprites map with direction
                        // Prefer action=0 (idle) frames over walk/attack frames
                        let key = AnimatedSpriteKey {
                            name: name.clone(),
                            index,
                            direction,
                        };

                        // Only insert if: no entry exists yet, OR this is an idle frame (action=0)
                        // This ensures we prefer idle frames over walking/attack frames
                        if action == 0 || !atlas.animated_sprites.contains_key(&key) {
                            atlas.animated_sprites.insert(key, sprite.clone());
                        }

                        // Also store direction 0 in the regular sheets for backwards compatibility
                        // Only store idle frames (action=0) or if no entry exists
                        if direction == 0 {
                            let sheet = atlas
                                .sheets
                                .entry(name)
                                .or_insert_with(SpriteSheet::default);
                            if action == 0 || !sheet.sprites.contains_key(&index) {
                                sheet.sprites.insert(index, sprite);
                            }
                        }
                    }
                }
            }
        }

        atlas.loaded = true;
        Ok(atlas)
    }

    /// Parse a single sprite sheet table.
    fn parse_sprite_sheet(
        data: &[u8],
        pos: usize,
    ) -> Result<(String, HashMap<i32, SpriteData>), SpriteAtlasError> {
        let vtable_offset = read_i32(data, pos);
        let vtable_pos = (pos as i32 - vtable_offset) as usize;
        let vtable_size = read_u16(data, vtable_pos) as usize;
        let num_fields = (vtable_size - 4) / 2;

        if num_fields < 3 {
            return Err(SpriteAtlasError::InvalidFormat(
                "SpriteSheet has too few fields".to_string(),
            ));
        }

        // According to the FlatBuffer schema:
        // Field 0 (offset 4): name (string)
        // Field 1 (offset 6): atlasId (long) - not used here
        // Field 2 (offset 8): sprites (vector)
        let name_field_offset = read_u16(data, vtable_pos + 4) as usize;
        let sprites_field_offset = read_u16(data, vtable_pos + 8) as usize;

        // Read name
        let name = if name_field_offset != 0 {
            let name_offset_pos = pos + name_field_offset;
            let name_offset = read_u32(data, name_offset_pos) as usize;
            let name_pos = name_offset_pos + name_offset;
            read_string(data, name_pos)?
        } else {
            return Err(SpriteAtlasError::InvalidFormat(
                "SpriteSheet missing name".to_string(),
            ));
        };

        // Read sprites vector
        let mut sprites = HashMap::new();
        if sprites_field_offset != 0 {
            let vector_offset_pos = pos + sprites_field_offset;
            let vector_offset = read_u32(data, vector_offset_pos) as usize;
            let vector_pos = vector_offset_pos + vector_offset;
            let num_sprites = read_u32(data, vector_pos) as usize;

            for i in 0..num_sprites {
                let sprite_offset_pos = vector_pos + 4 + (i * 4);
                let sprite_offset = read_u32(data, sprite_offset_pos) as usize;
                let sprite_pos = sprite_offset_pos + sprite_offset;

                if let Ok(sprite) = Self::parse_sprite(data, sprite_pos) {
                    sprites.insert(sprite.0, sprite.1);
                }
            }
        }

        Ok((name, sprites))
    }

    /// Parse a single sprite table.
    fn parse_sprite(data: &[u8], pos: usize) -> Result<(i32, SpriteData), SpriteAtlasError> {
        let vtable_offset = read_i32(data, pos);
        let vtable_pos = (pos as i32 - vtable_offset) as usize;
        let vtable_size = read_u16(data, vtable_pos) as usize;
        let num_fields = if vtable_size >= 4 {
            (vtable_size - 4) / 2
        } else {
            0
        };

        // Fields: position (0), maskPosition (1), padding (2), index (3), mostCommonColor (4), isT (5), spriteSheetName (6), aId (7)
        // We need: position (0), index (3), mostCommonColor (4), aId (7)

        let mut x = 0i32;
        let mut y = 0i32;
        let mut width = 8i32;
        let mut height = 8i32;
        let mut index = 0i32;
        let mut atlas_id = 1u8;
        let mut color = [255u8, 255, 255, 255];
        let mut mask: Option<MaskPosition> = None;

        // Position field (0) - struct inline
        if num_fields > 0 {
            let pos_field_offset = read_u16(data, vtable_pos + 4) as usize;
            if pos_field_offset != 0 {
                let pos_data_pos = pos + pos_field_offset;
                // Position struct: x (f32), y (f32), w (f32), h (f32)
                let x_f = read_f32(data, pos_data_pos);
                let y_f = read_f32(data, pos_data_pos + 4);
                width = read_f32(data, pos_data_pos + 8) as i32;
                height = read_f32(data, pos_data_pos + 12) as i32;
                x = x_f as i32;
                y = y_f as i32;
            }
        }

        // MaskPosition field (1) - struct inline (same layout as Position)
        if num_fields > 1 {
            let mask_field_offset = read_u16(data, vtable_pos + 6) as usize;
            if mask_field_offset != 0 {
                let mask_data_pos = pos + mask_field_offset;
                let mx = read_f32(data, mask_data_pos) as i32;
                let my = read_f32(data, mask_data_pos + 4) as i32;
                let mw = read_f32(data, mask_data_pos + 8) as i32;
                let mh = read_f32(data, mask_data_pos + 12) as i32;
                if mw > 0 && mh > 0 {
                    mask = Some(MaskPosition {
                        x: mx,
                        y: my,
                        width: mw,
                        height: mh,
                    });
                }
            }
        }

        // Index field (3)
        if num_fields > 3 {
            let index_field_offset = read_u16(data, vtable_pos + 4 + 6) as usize;
            if index_field_offset != 0 {
                index = read_i32(data, pos + index_field_offset);
            }
        }

        // MostCommonColor field (4) - struct inline
        if num_fields > 4 {
            let color_field_offset = read_u16(data, vtable_pos + 4 + 8) as usize;
            if color_field_offset != 0 {
                let color_pos = pos + color_field_offset;
                // Color struct: r (f32), g (f32), b (f32), a (f32)
                color[0] = (read_f32(data, color_pos) * 255.0) as u8;
                color[1] = (read_f32(data, color_pos + 4) * 255.0) as u8;
                color[2] = (read_f32(data, color_pos + 8) * 255.0) as u8;
                color[3] = (read_f32(data, color_pos + 12) * 255.0) as u8;
            }
        }

        // aId field (7)
        if num_fields > 7 {
            let aid_field_offset = read_u16(data, vtable_pos + 4 + 14) as usize;
            if aid_field_offset != 0 {
                atlas_id = read_i64(data, pos + aid_field_offset) as u8;
            }
        }

        Ok((
            index,
            SpriteData {
                x,
                y,
                width,
                height,
                atlas_id,
                color,
                mask,
            },
        ))
    }

    /// Parse an animated sprite entry.
    ///
    /// AnimatedSprite fields (from FlatBuffer schema):
    /// - Field 0 (offset 4): name - string
    /// - Field 1 (offset 6): index - i32 (stored/read as a 64-bit long)
    /// - Field 2 (offset 8): set - i32
    /// - Field 3 (offset 10): direction - i32
    /// - Field 4 (offset 12): action - i32 (0=idle, 1=walk, 2=attack)
    /// - Field 5 (offset 14): sprites - Sprite (table offset)
    ///
    /// Returns: (name, index, direction, action, SpriteData)
    fn parse_animated_sprite(
        data: &[u8],
        pos: usize,
    ) -> Result<(String, i32, i32, i32, SpriteData), SpriteAtlasError> {
        let vtable_offset = read_i32(data, pos);
        let vtable_pos = (pos as i32 - vtable_offset) as usize;
        let vtable_size = read_u16(data, vtable_pos) as usize;
        let num_fields = if vtable_size >= 4 {
            (vtable_size - 4) / 2
        } else {
            0
        };

        let mut name = String::new();
        let mut index = 0i32;
        let mut direction = 0i32;
        let mut action = 0i32;
        let mut sprite_data = SpriteData {
            x: 0,
            y: 0,
            width: 8,
            height: 8,
            atlas_id: 1,
            color: [255, 255, 255, 255],
            mask: None,
        };

        // Name field (0) - offset 4 in vtable
        if num_fields > 0 {
            let name_field_offset = read_u16(data, vtable_pos + 4) as usize;
            if name_field_offset != 0 {
                let name_offset_pos = pos + name_field_offset;
                let name_offset = read_u32(data, name_offset_pos) as usize;
                let name_pos = name_offset_pos + name_offset;
                name = read_string(data, name_pos)?;
            }
        }

        // Index field (1) - offset 6 in vtable
        if num_fields > 1 {
            let index_field_offset = read_u16(data, vtable_pos + 6) as usize;
            if index_field_offset != 0 {
                // Read as i32 (stored as a long but the underlying value is an int)
                index = read_i32(data, pos + index_field_offset);
            }
        }

        // Direction field (3) - offset 10 in vtable
        if num_fields > 3 {
            let dir_field_offset = read_u16(data, vtable_pos + 10) as usize;
            if dir_field_offset != 0 {
                direction = read_i32(data, pos + dir_field_offset);
            }
        }

        // Action field (4) - offset 12 in vtable (0=idle, 1=walk, 2=attack)
        if num_fields > 4 {
            let action_field_offset = read_u16(data, vtable_pos + 12) as usize;
            if action_field_offset != 0 {
                action = read_i32(data, pos + action_field_offset);
            }
        }

        // Sprites field (5) - offset 14 in vtable (NOT field 2!)
        if num_fields > 5 {
            let sprite_field_offset = read_u16(data, vtable_pos + 14) as usize;
            if sprite_field_offset != 0 {
                let sprite_offset_pos = pos + sprite_field_offset;
                let sprite_offset = read_u32(data, sprite_offset_pos) as usize;
                let sprite_pos = sprite_offset_pos + sprite_offset;
                if let Ok((_, sd)) = Self::parse_sprite(data, sprite_pos) {
                    sprite_data = sd;
                }
            }
        }

        Ok((name, index, direction, action, sprite_data))
    }

    /// Get sprite data by sheet name and index.
    /// Falls back to animated_sprites with any direction if not found in regular sheets.
    pub fn get_sprite(&self, sheet_name: &str, index: i32) -> Option<&SpriteData> {
        // First try the regular sheets
        if let Some(sheet) = self.sheets.get(sheet_name) {
            if let Some(sprite) = sheet.sprites.get(&index) {
                return Some(sprite);
            }
        }

        // Fall back to animated sprites - try direction 0 first, then 2 (side), then any
        for dir in [0, 2, 1, 3, 4, 5, 6, 7] {
            let key = AnimatedSpriteKey {
                name: sheet_name.to_string(),
                index,
                direction: dir,
            };
            if let Some(sprite) = self.animated_sprites.get(&key) {
                //tracing::debug!("Sprite '{}' index {} found via animated_sprites dir={}", sheet_name, index, dir);
                return Some(sprite);
            }
        }

        None
    }

    /// Get animated sprite data by sheet name, index, and direction.
    ///
    /// Direction values (RotMG convention):
    /// - 0: Down (facing screen)
    /// - 1: Down-Left
    /// - 2: Left
    /// - 3: Up-Left
    /// - 4: Up
    /// - 5: Up-Right
    /// - 6: Right
    /// - 7: Down-Right
    ///
    /// Falls back to the regular sprite (direction 0) if the requested direction isn't available.
    pub fn get_sprite_with_direction(
        &self,
        sheet_name: &str,
        index: i32,
        direction: i32,
    ) -> Option<&SpriteData> {
        // Try to get the sprite with the specific direction
        let key = AnimatedSpriteKey {
            name: sheet_name.to_string(),
            index,
            direction,
        };
        if let Some(sprite) = self.animated_sprites.get(&key) {
            return Some(sprite);
        }

        // Fall back to direction 0
        if direction != 0 {
            let key0 = AnimatedSpriteKey {
                name: sheet_name.to_string(),
                index,
                direction: 0,
            };
            if let Some(sprite) = self.animated_sprites.get(&key0) {
                return Some(sprite);
            }
        }

        // Fall back to regular sprite sheet
        self.get_sprite(sheet_name, index)
    }

    /// Check if sprites have been loaded.
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Get the number of sprite sheets.
    pub fn sheet_count(&self) -> usize {
        self.sheets.len()
    }

    /// Get the total number of sprites across all sheets.
    pub fn sprite_count(&self) -> usize {
        self.sheets.values().map(|s| s.sprites.len()).sum()
    }
}

/// Error type for sprite atlas operations.
#[derive(Debug)]
pub enum SpriteAtlasError {
    /// IO error reading file
    Io(std::io::Error),
    /// Invalid FlatBuffer format
    InvalidFormat(String),
}

impl From<std::io::Error> for SpriteAtlasError {
    fn from(e: std::io::Error) -> Self {
        SpriteAtlasError::Io(e)
    }
}

impl std::fmt::Display for SpriteAtlasError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpriteAtlasError::Io(e) => write!(f, "IO error: {}", e),
            SpriteAtlasError::InvalidFormat(msg) => write!(f, "Invalid format: {}", msg),
        }
    }
}

impl std::error::Error for SpriteAtlasError {}

// Helper functions for reading little-endian values
fn read_u16(data: &[u8], pos: usize) -> u16 {
    if pos + 2 > data.len() {
        return 0;
    }
    u16::from_le_bytes([data[pos], data[pos + 1]])
}

fn read_u32(data: &[u8], pos: usize) -> u32 {
    if pos + 4 > data.len() {
        return 0;
    }
    u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
}

fn read_i32(data: &[u8], pos: usize) -> i32 {
    if pos + 4 > data.len() {
        return 0;
    }
    i32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
}

fn read_i64(data: &[u8], pos: usize) -> i64 {
    if pos + 8 > data.len() {
        return 0;
    }
    i64::from_le_bytes([
        data[pos],
        data[pos + 1],
        data[pos + 2],
        data[pos + 3],
        data[pos + 4],
        data[pos + 5],
        data[pos + 6],
        data[pos + 7],
    ])
}

fn read_f32(data: &[u8], pos: usize) -> f32 {
    if pos + 4 > data.len() {
        return 0.0;
    }
    f32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
}

fn read_string(data: &[u8], pos: usize) -> Result<String, SpriteAtlasError> {
    // String format: [length: u32, utf8_bytes...]
    if pos + 4 > data.len() {
        return Err(SpriteAtlasError::InvalidFormat(
            "String position out of bounds".to_string(),
        ));
    }
    let len = read_u32(data, pos) as usize;
    if pos + 4 + len > data.len() {
        return Err(SpriteAtlasError::InvalidFormat(
            "String length exceeds buffer".to_string(),
        ));
    }
    String::from_utf8(data[pos + 4..pos + 4 + len].to_vec())
        .map_err(|_| SpriteAtlasError::InvalidFormat("Invalid UTF-8 string".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resolve the source assets directory for the ignored manual export tests
    /// from an explicit env var. Returns `None` (skip) when unset -- never falls
    /// back to a developer machine's LOCALAPPDATA.
    fn export_test_assets_dir() -> Option<std::path::PathBuf> {
        std::env::var_os("REALMHOUND_TEST_ASSETS_DIR").map(std::path::PathBuf::from)
    }

    /// Resolve the output directory for the ignored manual export tests: an
    /// explicit env var, otherwise a unique temporary directory. Never deletes
    /// existing data.
    fn export_test_output_dir(name: &str) -> std::path::PathBuf {
        let base = std::env::var_os("REALMHOUND_TEST_EXPORT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        base.join(format!("realmhound_export_{}_{}", name, std::process::id()))
    }

    #[test]
    fn test_sprite_data_region() {
        let sprite = SpriteData {
            x: 10,
            y: 20,
            width: 8,
            height: 8,
            atlas_id: 1,
            color: [255, 0, 0, 255],
            mask: None,
        };
        assert_eq!(sprite.region(), (10, 20, 8, 8));
    }

    #[test]
    fn test_empty_atlas() {
        let atlas = SpriteAtlas::new();
        assert!(!atlas.is_loaded());
        assert_eq!(atlas.sheet_count(), 0);
        assert!(atlas.get_sprite("test", 0).is_none());
    }

    #[test]
    #[ignore] // Run manually: cargo test test_sprite_sheets -- --ignored --nocapture
    fn test_sprite_sheets() {
        let Some(assets_dir) = export_test_assets_dir() else {
            println!("set REALMHOUND_TEST_ASSETS_DIR to run this manual export test");
            return;
        };
        let spritesheet_path = assets_dir.join("flatbuffer").join("spritesheetf");

        if !spritesheet_path.exists() {
            println!("Spritesheet not found at {:?}", spritesheet_path);
            return;
        }

        let atlas = SpriteAtlas::load_from_file(&spritesheet_path).expect("Failed to load atlas");

        println!(
            "Loaded {} sheets with {} sprites total",
            atlas.sheet_count(),
            atlas.sprite_count()
        );
        println!("\nSheet names:");
        for name in atlas.sheets.keys() {
            let sheet = atlas.sheets.get(name).unwrap();
            println!("  {} ({} sprites)", name, sheet.sprites.len());
        }

        // Try to find some specific sprites
        let test_sheets = ["lofiObj", "lofiObj2", "lofiObj3", "d3LofiObjEmbed"];
        for sheet_name in test_sheets {
            println!("\nLooking for sheet: {}", sheet_name);
            if let Some(sheet) = atlas.sheets.get(sheet_name) {
                println!(
                    "  Found! {} sprites. First few indices: {:?}",
                    sheet.sprites.len(),
                    sheet.sprites.keys().take(5).collect::<Vec<_>>()
                );
            } else {
                println!("  NOT FOUND");
            }
        }
    }

    #[test]
    #[ignore] // Run manually: cargo test export_lofi_sprites -- --ignored --nocapture
    fn export_lofi_sprites() {
        use std::fs;

        let Some(assets_dir) = export_test_assets_dir() else {
            println!("set REALMHOUND_TEST_ASSETS_DIR to run this manual export test");
            return;
        };
        let spritesheet_path = assets_dir.join("flatbuffer").join("spritesheetf");

        if !spritesheet_path.exists() {
            println!("Spritesheet not found at {:?}", spritesheet_path);
            return;
        }

        // Load sprite coordinate data
        let atlas = SpriteAtlas::load_from_file(&spritesheet_path).expect("Failed to load atlas");
        println!(
            "Loaded {} sheets with {} sprites total",
            atlas.sheet_count(),
            atlas.sprite_count()
        );

        // Map atlas_id to PNG filename
        let atlas_files = [
            (1, "groundTiles"),
            (2, "characters"),
            (3, "characters_masks"),
            (4, "mapObjects"),
        ];

        // Load atlas PNG images
        let mut atlas_images: HashMap<u8, image::DynamicImage> = HashMap::new();
        for (id, name) in atlas_files {
            let path = assets_dir.join("sprites").join(format!("{}.png", name));
            if path.exists() {
                match image::open(&path) {
                    Ok(img) => {
                        println!(
                            "Loaded atlas {}: {} ({}x{})",
                            id,
                            name,
                            img.width(),
                            img.height()
                        );
                        atlas_images.insert(id, img);
                    }
                    Err(e) => println!("Failed to load {}: {}", name, e),
                }
            }
        }

        // Create output directory
        let output_dir = export_test_output_dir("lofi_sprites");
        fs::create_dir_all(&output_dir).expect("Failed to create output dir");

        // Export sprites from specific sheets
        let sheets_to_export = [
            "lofiObj",
            "lofiObj2",
            "lofiObj3",
            "lofiObj4",
            "lofiObj5",
            "lofiObj6",
            "lofiObjBig",
            "lofiChar8x8",
            "lofiEnvironment",
            "lofiEnvironment2",
            "d2LofiObjEmbed",
            "d3LofiObjEmbed",
        ];

        for sheet_name in sheets_to_export {
            if let Some(sheet) = atlas.sheets.get(sheet_name) {
                let sheet_dir = output_dir.join(sheet_name);
                fs::create_dir_all(&sheet_dir).expect("Failed to create sheet dir");

                // Collect all cropped sprites for atlas generation
                let mut cropped_sprites: Vec<(i32, image::DynamicImage, u32, u32)> = Vec::new();

                let mut exported = 0;
                for (&index, sprite) in &sheet.sprites {
                    if let Some(atlas_img) = atlas_images.get(&sprite.atlas_id) {
                        // Crop sprite from atlas
                        let x = sprite.x as u32;
                        let y = sprite.y as u32;
                        let w = sprite.width as u32;
                        let h = sprite.height as u32;

                        // Ensure bounds are valid
                        if x + w <= atlas_img.width() && y + h <= atlas_img.height() {
                            let cropped = atlas_img.crop_imm(x, y, w, h);
                            let out_path = sheet_dir.join(format!("{}.png", index));
                            if cropped.save(&out_path).is_ok() {
                                exported += 1;
                                cropped_sprites.push((index, cropped, w, h));
                            }
                        }
                    }
                }

                // Generate combined atlas.png for this sheet
                if !cropped_sprites.is_empty() {
                    // Sort by index for consistent layout
                    cropped_sprites.sort_by_key(|(idx, _, _, _)| *idx);

                    // Find max sprite dimensions and calculate grid
                    let max_w = cropped_sprites
                        .iter()
                        .map(|(_, _, w, _)| *w)
                        .max()
                        .unwrap_or(8);
                    let max_h = cropped_sprites
                        .iter()
                        .map(|(_, _, _, h)| *h)
                        .max()
                        .unwrap_or(8);
                    let cell_size = max_w.max(max_h) + 2; // Add 2px padding

                    // Calculate grid dimensions (aim for roughly square)
                    let count = cropped_sprites.len();
                    let cols = (count as f64).sqrt().ceil() as u32;
                    let rows = ((count as u32) + cols - 1) / cols;

                    // Create atlas image
                    let atlas_width = cols * cell_size;
                    let atlas_height = rows * cell_size;
                    let mut combined = image::RgbaImage::new(atlas_width, atlas_height);

                    // Place sprites in grid
                    for (i, (_, sprite_img, _, _)) in cropped_sprites.iter().enumerate() {
                        let col = (i as u32) % cols;
                        let row = (i as u32) / cols;
                        let dest_x = col * cell_size + 1; // 1px padding
                        let dest_y = row * cell_size + 1;

                        // Copy sprite pixels to atlas
                        let rgba = sprite_img.to_rgba8();
                        for (px, py, pixel) in rgba.enumerate_pixels() {
                            if dest_x + px < atlas_width && dest_y + py < atlas_height {
                                combined.put_pixel(dest_x + px, dest_y + py, *pixel);
                            }
                        }
                    }

                    // Save combined atlas
                    let atlas_path = sheet_dir.join("atlas.png");
                    if combined.save(&atlas_path).is_ok() {
                        println!(
                            "Exported {} sprites from {} (atlas: {}x{})",
                            exported, sheet_name, atlas_width, atlas_height
                        );
                    } else {
                        println!(
                            "Exported {} sprites from {} (atlas save failed)",
                            exported, sheet_name
                        );
                    }
                } else {
                    println!("Exported {} sprites from {}", exported, sheet_name);
                }
            }
        }

        println!("\nSprites exported to: {:?}", output_dir);
    }

    #[test]
    #[ignore] // Run manually: cargo test export_chat_tab_icon -- --ignored --nocapture
    fn export_chat_tab_icon() {
        let Some(assets_dir) = export_test_assets_dir() else {
            println!("set REALMHOUND_TEST_ASSETS_DIR to run this manual export test");
            return;
        };
        let spritesheet_path = assets_dir.join("flatbuffer").join("spritesheetf");
        if !spritesheet_path.exists() {
            println!("Spritesheet not found at {:?}", spritesheet_path);
            return;
        }

        let atlas = SpriteAtlas::load_from_file(&spritesheet_path).expect("Failed to load atlas");
        let sprite = atlas
            .get_sprite("lofiObj", 515)
            .copied()
            .expect("lofiObj:515 not found");

        let atlas_files = [
            (1u8, "groundTiles"),
            (2, "characters"),
            (3, "characters_masks"),
            (4, "mapObjects"),
        ];
        let (_, atlas_name) = atlas_files
            .iter()
            .find(|(id, _)| *id == sprite.atlas_id)
            .expect("unknown atlas_id");
        let atlas_img = image::open(
            assets_dir
                .join("sprites")
                .join(format!("{}.png", atlas_name)),
        )
        .expect("failed to open atlas png");

        let cropped = atlas_img.crop_imm(
            sprite.x as u32,
            sprite.y as u32,
            sprite.width as u32,
            sprite.height as u32,
        );

        let out_dir = export_test_output_dir("chat_tab_icon");
        std::fs::create_dir_all(&out_dir).expect("failed to create output dir");
        let native = out_dir.join("chat_tab_icon.png");
        cropped.save(&native).expect("save native failed");

        let scale = 32u32;
        let big = image::imageops::resize(
            &cropped.to_rgba8(),
            sprite.width as u32 * scale,
            sprite.height as u32 * scale,
            image::imageops::FilterType::Nearest,
        );
        let big_path = out_dir.join("chat_tab_icon_x32.png");
        big.save(&big_path).expect("save upscaled failed");

        println!(
            "Chat tab icon: {}x{} native -> {:?} and {:?}",
            sprite.width, sprite.height, native, big_path
        );
    }

    #[test]
    #[ignore] // Run manually: cargo test list_atlas_sheets -- --ignored --nocapture
    fn list_atlas_sheets() {
        let Some(assets_dir) = export_test_assets_dir() else {
            println!("set REALMHOUND_TEST_ASSETS_DIR to run this manual export test");
            return;
        };
        let spritesheet_path = assets_dir.join("flatbuffer").join("spritesheetf");

        if !spritesheet_path.exists() {
            println!("Spritesheet not found at {:?}", spritesheet_path);
            return;
        }

        let atlas = SpriteAtlas::load_from_file(&spritesheet_path).expect("Failed to load atlas");

        // Group sheets by their atlas_id
        let atlas_names = [
            (1, "groundTiles"),
            (2, "characters"),
            (3, "characters_masks"),
            (4, "mapObjects"),
        ];

        for (atlas_id, atlas_name) in atlas_names {
            println!("\n=== Atlas {}: {} ===", atlas_id, atlas_name);
            let mut sheets: Vec<_> = atlas
                .sheets
                .iter()
                .filter(|(_, sheet)| sheet.sprites.values().any(|s| s.atlas_id == atlas_id))
                .map(|(name, sheet)| {
                    let count = sheet
                        .sprites
                        .values()
                        .filter(|s| s.atlas_id == atlas_id)
                        .count();
                    (name.clone(), count)
                })
                .collect();
            sheets.sort_by(|a, b| b.1.cmp(&a.1)); // Sort by sprite count descending

            for (name, count) in sheets {
                println!("  {} ({} sprites)", name, count);
            }
        }
    }

    #[test]
    #[ignore] // Run manually: cargo test export_mapobjects_subatlases -- --ignored --nocapture
    fn export_mapobjects_subatlases() {
        use std::fs;

        let Some(assets_dir) = export_test_assets_dir() else {
            println!("set REALMHOUND_TEST_ASSETS_DIR to run this manual export test");
            return;
        };
        let spritesheet_path = assets_dir.join("flatbuffer").join("spritesheetf");

        if !spritesheet_path.exists() {
            println!("Spritesheet not found at {:?}", spritesheet_path);
            return;
        }

        // Load sprite coordinate data
        let atlas = SpriteAtlas::load_from_file(&spritesheet_path).expect("Failed to load atlas");
        println!(
            "Loaded {} sheets with {} sprites total",
            atlas.sheet_count(),
            atlas.sprite_count()
        );

        // Load mapObjects atlas (id=4)
        let mapobjects_path = assets_dir.join("sprites").join("mapObjects.png");
        if !mapobjects_path.exists() {
            println!("mapObjects.png not found at {:?}", mapobjects_path);
            return;
        }

        let mapobjects_img = image::open(&mapobjects_path).expect("Failed to load mapObjects.png");
        println!(
            "Loaded mapObjects.png ({}x{})",
            mapobjects_img.width(),
            mapobjects_img.height()
        );

        // Create output directory
        let output_dir = export_test_output_dir("mapobjects_subatlases");
        fs::create_dir_all(&output_dir).expect("Failed to create output dir");

        // Find all sheets that have sprites in mapObjects (atlas_id=4)
        let mut sheets_to_export: Vec<_> = atlas
            .sheets
            .iter()
            .filter(|(_, sheet)| sheet.sprites.values().any(|s| s.atlas_id == 4))
            .map(|(name, sheet)| {
                let count = sheet.sprites.values().filter(|s| s.atlas_id == 4).count();
                (name.clone(), count)
            })
            .collect();
        sheets_to_export.sort_by(|a, b| b.1.cmp(&a.1)); // Sort by sprite count descending

        println!(
            "\nExporting {} sub-atlases from mapObjects...",
            sheets_to_export.len()
        );

        for (sheet_name, _) in &sheets_to_export {
            let sheet = atlas.sheets.get(sheet_name).unwrap();

            // Collect sprites from this sheet that are in mapObjects atlas
            let mut sprites_to_export: Vec<_> = sheet
                .sprites
                .iter()
                .filter(|(_, sprite)| sprite.atlas_id == 4)
                .map(|(&index, sprite)| (index, sprite))
                .collect();
            sprites_to_export.sort_by_key(|(idx, _)| *idx);

            if sprites_to_export.is_empty() {
                continue;
            }

            // Find max sprite dimensions
            let max_w = sprites_to_export
                .iter()
                .map(|(_, s)| s.width as u32)
                .max()
                .unwrap_or(8);
            let max_h = sprites_to_export
                .iter()
                .map(|(_, s)| s.height as u32)
                .max()
                .unwrap_or(8);
            let cell_size = max_w.max(max_h) + 2; // Add 2px padding

            // Calculate grid dimensions (aim for roughly square)
            let count = sprites_to_export.len();
            let cols = (count as f64).sqrt().ceil() as u32;
            let rows = ((count as u32) + cols - 1) / cols;

            // Create combined atlas image
            let atlas_width = cols * cell_size;
            let atlas_height = rows * cell_size;
            let mut combined = image::RgbaImage::new(atlas_width, atlas_height);

            // Place sprites in grid
            for (i, (_, sprite)) in sprites_to_export.iter().enumerate() {
                let col = (i as u32) % cols;
                let row = (i as u32) / cols;
                let dest_x = col * cell_size + 1; // 1px padding
                let dest_y = row * cell_size + 1;

                // Crop sprite from mapObjects atlas
                let x = sprite.x as u32;
                let y = sprite.y as u32;
                let w = sprite.width as u32;
                let h = sprite.height as u32;

                if x + w <= mapobjects_img.width() && y + h <= mapobjects_img.height() {
                    let cropped = mapobjects_img.crop_imm(x, y, w, h).to_rgba8();
                    for (px, py, pixel) in cropped.enumerate_pixels() {
                        if dest_x + px < atlas_width && dest_y + py < atlas_height {
                            combined.put_pixel(dest_x + px, dest_y + py, *pixel);
                        }
                    }
                }
            }

            // Save combined atlas with sprite count in filename
            let out_path = output_dir.join(format!("{}_({}).png", sheet_name, count));
            if combined.save(&out_path).is_ok() {
                println!(
                    "  {} - {} sprites ({}x{})",
                    sheet_name, count, atlas_width, atlas_height
                );
            }
        }

        println!("\nSub-atlases exported to: {:?}", output_dir);
    }

    #[test]
    #[ignore] // Run manually: cargo test export_beacon_sprites -- --ignored --nocapture
    fn export_beacon_sprites() {
        use std::fs;

        let Some(assets_dir) = export_test_assets_dir() else {
            println!("set REALMHOUND_TEST_ASSETS_DIR to run this manual export test");
            return;
        };
        let spritesheet_path = assets_dir.join("flatbuffer").join("spritesheetf");

        if !spritesheet_path.exists() {
            println!("Spritesheet not found at {:?}", spritesheet_path);
            return;
        }

        // Load sprite coordinate data
        let atlas = SpriteAtlas::load_from_file(&spritesheet_path).expect("Failed to load atlas");
        println!(
            "Loaded {} sheets with {} sprites total",
            atlas.sheet_count(),
            atlas.sprite_count()
        );

        // Get the beacons32x32 sheet
        let sheet = match atlas.sheets.get("beacons32x32") {
            Some(s) => s,
            None => {
                println!("beacons32x32 sheet not found in atlas");
                return;
            }
        };

        println!(
            "Found beacons32x32 sheet with {} sprites, atlas_id: {}",
            sheet.sprites.len(),
            sheet
                .sprites
                .values()
                .next()
                .map(|s| s.atlas_id)
                .unwrap_or(0)
        );

        // Load the mapObjects atlas (atlas_id 4) which contains beacons
        let mapobjects_path = assets_dir.join("sprites").join("mapObjects.png");
        let mapobjects_img = match image::open(&mapobjects_path) {
            Ok(img) => {
                println!("Loaded mapObjects.png: {}x{}", img.width(), img.height());
                img
            }
            Err(e) => {
                println!("Failed to load mapObjects.png: {}", e);
                return;
            }
        };

        // Create output directory
        let output_dir = export_test_output_dir("beacon_sprites");
        fs::create_dir_all(&output_dir).expect("Failed to create output dir");

        // Load ObjectID.list to get beacon names by sprite index
        // Build a map: sprite_index -> (display_name, biome_name)
        // We want "Actual Active Beacon *" entries (the main beacon sprite, not animation frames)
        let object_list_path = assets_dir.join("ObjectID.list");
        let mut index_to_names: std::collections::HashMap<i32, (String, String)> =
            std::collections::HashMap::new();

        if object_list_path.exists() {
            use std::io::{BufRead, BufReader};
            let file = std::fs::File::open(&object_list_path).unwrap();
            let reader = BufReader::new(file);

            for line in reader.lines().flatten() {
                // Skip captured beacons
                if line.to_lowercase().contains("captured") {
                    continue;
                }
                // Check if line contains beacons32x32 texture
                if line.contains("beacons32x32") {
                    let parts: Vec<&str> = line.split(';').collect();
                    if parts.len() >= 8 {
                        let id_name = parts[7].trim();
                        let display_name = parts[1].trim();

                        // Only use "Actual Active Beacon *" entries (the static icon, not animation frames)
                        if !id_name.starts_with("Actual Active Beacon ") {
                            continue;
                        }

                        // Parse texture data to get sprite index
                        let texture_data = parts[5];
                        for tex in texture_data.split(',') {
                            let tex_parts: Vec<&str> = tex.split(':').collect();
                            if tex_parts.len() >= 2 && tex_parts[0] == "beacons32x32" {
                                let sprite_index = if tex_parts[1].starts_with("0x")
                                    || tex_parts[1].starts_with("0X")
                                {
                                    i32::from_str_radix(&tex_parts[1][2..], 16).unwrap_or(-1)
                                } else {
                                    tex_parts[1].parse().unwrap_or(-1)
                                };

                                if sprite_index >= 0 {
                                    // Extract clean biome name from "Actual Active Beacon Forest" -> "Forest"
                                    let biome_name = id_name
                                        .strip_prefix("Actual Active Beacon ")
                                        .unwrap_or(id_name);

                                    if !index_to_names.contains_key(&sprite_index) {
                                        index_to_names.insert(
                                            sprite_index,
                                            (display_name.to_string(), biome_name.to_string()),
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        println!(
            "Found {} unique beacon biomes from ObjectID.list",
            index_to_names.len()
        );

        // Export only the beacons we have names for (one per biome)
        let mut exported = 0;
        let mut exported_biomes: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for (&index, (display_name, biome_name)) in &index_to_names {
            // Skip if we already exported this biome
            if exported_biomes.contains(biome_name) {
                continue;
            }

            if let Some(sprite) = sheet.sprites.get(&index) {
                // Crop sprite from atlas
                let x = sprite.x as u32;
                let y = sprite.y as u32;
                let w = sprite.width as u32;
                let h = sprite.height as u32;

                if x + w <= mapobjects_img.width() && y + h <= mapobjects_img.height() {
                    let cropped = mapobjects_img.crop_imm(x, y, w, h);

                    // Use biome name, replacing spaces with underscores
                    let clean_name: String = biome_name
                        .chars()
                        .map(|c| {
                            if c.is_alphanumeric() || c == '-' {
                                c
                            } else {
                                '_'
                            }
                        })
                        .collect();
                    let filename = format!("{}.png", clean_name);
                    let out_path = output_dir.join(&filename);

                    if cropped.save(&out_path).is_ok() {
                        exported += 1;
                        exported_biomes.insert(biome_name.clone());
                        println!(
                            "Exported \"{}\" ({}) -> {}",
                            display_name, biome_name, filename
                        );
                    }
                }
            }
        }

        println!(
            "\nExported {} unique beacon sprites to: {:?}",
            exported, output_dir
        );
    }
}

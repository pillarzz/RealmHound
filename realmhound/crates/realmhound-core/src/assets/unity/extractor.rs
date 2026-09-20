//! Main Unity asset extractor.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::object_reader::ClassIDType;
use super::serialized_file::SerializedFile;
use super::text_asset::TextAsset;
use super::texture2d::{Texture2D, STREAMED_TEXTURE_NAMES};
use super::xml_parser::generate_asset_lists;

/// Result of asset extraction.
#[derive(Debug, Default)]
pub struct ExtractionResult {
    pub xml_files: usize,
    pub sprites: usize,
    pub spritesheet: bool,
    pub objects: usize,
    pub tiles: usize,
    pub errors: Vec<String>,
}

/// Unity asset extractor for RotMG resources.assets.
pub struct UnityExtractor {
    /// Track duplicate names
    seen_names: HashSet<String>,
}

impl UnityExtractor {
    /// Create a new extractor.
    pub fn new() -> Self {
        Self {
            seen_names: HashSet::new(),
        }
    }

    /// Extract assets from resources.assets to the output directory.
    ///
    /// Creates the following structure:
    /// - output_dir/flatbuffer/spritesheetf
    /// - output_dir/sprites/*.png
    /// - output_dir/xml/*.xml
    pub fn extract<P: AsRef<Path>, Q: AsRef<Path>>(
        &mut self,
        input: P,
        output_dir: Q,
    ) -> io::Result<ExtractionResult> {
        let output = output_dir.as_ref();
        let mut result = ExtractionResult::default();

        // Create output directories
        fs::create_dir_all(output.join("flatbuffer"))?;
        fs::create_dir_all(output.join("sprites"))?;
        fs::create_dir_all(output.join("xml"))?;

        // Directory containing the serialized file, used to resolve external
        // streaming resources (.resS) for streamed textures.
        let input_path = input.as_ref().to_path_buf();
        let base_dir = input_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();

        // Parse the serialized file
        let mut sf = SerializedFile::parse(&input_path)?;

        // Collect object info we need (to avoid borrow issues)
        let objects_info: Vec<_> = sf
            .objects
            .iter()
            .map(|obj| (obj.class_type, obj.byte_start, obj.byte_size))
            .collect();

        // Process all objects
        for (i, &(class_type, byte_start, byte_size)) in objects_info.iter().enumerate() {
            // Create a minimal ObjectInfo for parsing
            let obj_info = super::object_reader::ObjectInfo {
                path_id: 0,
                byte_start,
                byte_size,
                type_id: 0,
                class_id: 0,
                class_type,
            };

            match class_type {
                ClassIDType::TextAsset => {
                    match TextAsset::parse(sf.reader_mut(), &obj_info) {
                        Ok(asset) => {
                            if asset.is_spritesheet() {
                                // Save spritesheet flatbuffer
                                let path = output.join("flatbuffer/spritesheetf");
                                if let Err(e) = fs::write(&path, &asset.data) {
                                    result
                                        .errors
                                        .push(format!("Failed to write spritesheetf: {}", e));
                                } else {
                                    result.spritesheet = true;
                                }
                            } else if asset.is_xml() {
                                // Save XML file
                                let name = self.unique_name(&asset.name);
                                let path = output.join(format!("xml/{}.xml", name));
                                if let Err(e) = fs::write(&path, &asset.data) {
                                    result
                                        .errors
                                        .push(format!("Failed to write {}.xml: {}", name, e));
                                } else {
                                    result.xml_files += 1;
                                }
                            }
                        }
                        Err(e) => {
                            result
                                .errors
                                .push(format!("Failed to parse TextAsset {}: {}", i, e));
                        }
                    }
                }
                ClassIDType::Texture2D => {
                    match Texture2D::parse(sf.reader_mut(), &obj_info) {
                        Ok(mut texture) => {
                            if texture.is_spritesheet() {
                                // Save sprite PNG
                                match texture.to_png() {
                                    Ok(png_data) => {
                                        let path =
                                            output.join(format!("sprites/{}.png", texture.name));
                                        if let Err(e) = fs::write(&path, &png_data) {
                                            result.errors.push(format!(
                                                "Failed to write {}.png: {}",
                                                texture.name, e
                                            ));
                                        } else {
                                            result.sprites += 1;
                                        }
                                    }
                                    Err(e) => {
                                        result.errors.push(format!(
                                            "Failed to convert {} to PNG: {}",
                                            texture.name, e
                                        ));
                                    }
                                }
                            } else if STREAMED_TEXTURE_NAMES.contains(&texture.name.as_str())
                                && texture.is_streamed()
                            {
                                // Streamed texture: load external .resS bytes, then encode.
                                let name = texture.name.clone();
                                match texture
                                    .load_streamed_data(&base_dir)
                                    .and_then(|_| texture.to_png())
                                {
                                    Ok(png_data) => {
                                        let path = output.join(format!("sprites/{}.png", name));
                                        if let Err(e) = fs::write(&path, &png_data) {
                                            result.errors.push(format!(
                                                "Failed to write {}.png: {}",
                                                name, e
                                            ));
                                        } else {
                                            result.sprites += 1;
                                        }
                                    }
                                    Err(e) => {
                                        result.errors.push(format!(
                                            "Failed to extract streamed texture {}: {}",
                                            name, e
                                        ));
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            result
                                .errors
                                .push(format!("Failed to parse Texture2D {}: {}", i, e));
                        }
                    }
                }
                _ => {}
            }
        }

        // Generate ObjectID.list and TileID.list from extracted XML
        let xml_dir = output.join("xml");
        match generate_asset_lists(&xml_dir, output) {
            Ok((objects, tiles)) => {
                result.objects = objects;
                result.tiles = tiles;
                tracing::info!("Generated lists: {} objects, {} tiles", objects, tiles);
            }
            Err(e) => {
                result
                    .errors
                    .push(format!("Failed to generate asset lists: {}", e));
            }
        }

        Ok(result)
    }

    /// Generate a unique name (handling duplicates).
    fn unique_name(&mut self, name: &str) -> String {
        let mut unique = name.to_string();
        let mut count = 1;

        while self.seen_names.contains(&unique) {
            count += 1;
            unique = format!("{}{}", name, count);
        }

        self.seen_names.insert(unique.clone());
        unique
    }
}

impl Default for UnityExtractor {
    fn default() -> Self {
        Self::new()
    }
}

/// Find the default RotMG resources.assets path.
///
/// Checks multiple locations in priority order (newest first):
/// 1. `%LOCALAPPDATA%\RealmOfTheMadGod\Production\` (current game location since ~2025)
/// 2. `%USERPROFILE%\Documents\RealmOfTheMadGod\Production\` (legacy location)
///
/// When both exist, the most recently modified file wins, since the legacy
/// Documents location may still contain a stale copy.
pub fn find_resources_assets() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    #[cfg(target_os = "windows")]
    {
        // New location: AppData\Local (game moved here in a recent update)
        if let Some(local_app_data) = dirs::data_local_dir() {
            let path = local_app_data
                .join("RealmOfTheMadGod/Production/RotMG Exalt_Data/resources.assets");
            if path.exists() {
                candidates.push(path);
            }
        }

        // Legacy location: Documents folder
        if let Some(docs) = dirs::document_dir() {
            let path = docs.join("RealmOfTheMadGod/Production/RotMG Exalt_Data/resources.assets");
            if path.exists() {
                candidates.push(path);
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(docs) = dirs::document_dir() {
            let path = docs.join("RealmOfTheMadGod/Production/RotMGExalt.app/Contents/Resources/Data/resources.assets");
            if path.exists() {
                candidates.push(path);
            }
        }
    }

    if candidates.is_empty() {
        return None;
    }

    // If multiple candidates, pick the most recently modified one
    if candidates.len() > 1 {
        candidates.sort_by(|a, b| {
            let mtime_a = std::fs::metadata(a).and_then(|m| m.modified()).ok();
            let mtime_b = std::fs::metadata(b).and_then(|m| m.modified()).ok();
            mtime_b.cmp(&mtime_a) // newest first
        });
        tracing::info!(
            "[ASSETS] Multiple resources.assets found, using newest: {:?}",
            candidates[0]
        );
    }

    Some(candidates.remove(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore] // Run manually: cargo test test_extract_assets -- --ignored --nocapture
    fn test_extract_assets() {
        let resources_path =
            find_resources_assets().expect("Could not find resources.assets - is RotMG installed?");

        println!("Found resources.assets at: {:?}", resources_path);

        // Extract to a temp directory
        let output_dir = std::env::temp_dir().join("RealmHound_test_assets");
        let _ = fs::remove_dir_all(&output_dir); // Clean up from previous runs

        let mut extractor = UnityExtractor::new();
        let result = extractor
            .extract(&resources_path, &output_dir)
            .expect("Extraction failed");

        println!("Extraction complete:");
        println!("  XML files: {}", result.xml_files);
        println!("  Sprites: {}", result.sprites);
        println!("  Spritesheet: {}", result.spritesheet);
        println!("  Objects: {}", result.objects);
        println!("  Tiles: {}", result.tiles);

        if !result.errors.is_empty() {
            println!("  Errors:");
            for err in &result.errors {
                println!("    - {}", err);
            }
        }

        // Verify expected files exist
        assert!(
            output_dir.join("flatbuffer/spritesheetf").exists(),
            "Spritesheet should exist"
        );
        assert!(
            output_dir.join("sprites/characters.png").exists(),
            "characters.png should exist"
        );
        assert!(
            output_dir.join("ObjectID.list").exists(),
            "ObjectID.list should exist"
        );
        assert!(
            output_dir.join("TileID.list").exists(),
            "TileID.list should exist"
        );
        assert!(result.xml_files > 0, "Should have extracted some XML files");
        assert!(result.objects > 0, "Should have parsed some objects");

        println!("\nAssets extracted to: {:?}", output_dir);
    }
}

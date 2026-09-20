//! Unity asset bundle extractor for RotMG resources.assets.
//!
//! This module provides functionality to extract game assets from Unity's
//! binary asset bundle format (resources.assets). The extracted assets include:
//! - XML game data (objects, tiles, etc.)
//! - Sprite atlas PNGs
//! - FlatBuffer sprite coordinate data
//! - ObjectID.list and TileID.list generated from XML
//!
//! # Format Overview
//!
//! The resources.assets file is a Unity SerializedFile containing:
//! - File header with version and offset information
//! - Type metadata for object serialization
//! - Object table with offsets to actual data
//! - Serialized object data (TextAsset, Texture2D, etc.)
//!
//! Based on UnityPy: https://github.com/K0lb3/UnityPy

mod extractor;
mod file_header;
pub mod object_reader;
mod reader;
mod serialized_file;
mod serialized_type;
mod text_asset;
mod texture2d;
pub mod xml_parser;

pub use extractor::{find_resources_assets, ExtractionResult, UnityExtractor};
pub use file_header::FileHeader;
pub use xml_parser::generate_asset_lists;

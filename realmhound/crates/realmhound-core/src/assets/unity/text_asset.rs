//! TextAsset parsing.

use super::object_reader::ObjectInfo;
use super::reader::DataReader;
use std::io;

/// Non-XML files that shouldn't be saved as .xml
pub const NON_XML_FILES: &[&str] = &[
    "manifest_xml",
    "COPYING",
    "Errors",
    "ExplainUnzip",
    "cloth_bazaar",
    "Cursors",
    "Dialogs",
    "Keyboard",
    "LICENSE",
    "LineBreaking Following Characters",
    "LineBreaking Leading Characters",
    "manifest_json",
    "spritesheetf",
    "iso_4217",
    "data",
    "manifest",
    "BillingMode",
];

/// Parsed TextAsset from Unity.
#[derive(Debug)]
pub struct TextAsset {
    pub name: String,
    pub data: Vec<u8>,
}

impl TextAsset {
    /// Parse a TextAsset from the reader at the object's position.
    pub fn parse(reader: &mut DataReader, obj: &ObjectInfo) -> io::Result<Self> {
        reader.set_position(obj.byte_start)?;

        let name = reader.read_aligned_string()?;
        let data = reader.read_byte_array()?;

        Ok(Self { name, data })
    }

    /// Check if this is the spritesheet flatbuffer file.
    pub fn is_spritesheet(&self) -> bool {
        self.name == "spritesheetf"
    }

    /// Check if this should be saved as an XML file.
    pub fn is_xml(&self) -> bool {
        !NON_XML_FILES.contains(&self.name.as_str())
    }
}

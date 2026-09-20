//! Unity SerializedFile parser.

use std::fs::File;
use std::io;
use std::path::Path;

use super::file_header::{FileHeader, FileType};
use super::object_reader::ObjectInfo;
use super::reader::DataReader;
use super::serialized_type::SerializedType;

/// Platform enumeration.
#[derive(Debug, Clone, Copy)]
#[repr(i32)]
#[allow(dead_code)]
pub enum Platform {
    Unknown = -1,
    StandaloneWinPlayer = 5,
    StandaloneOSX = 4,
    StandaloneLinux64 = 17,
    WebGL = 20,
}

impl From<i32> for Platform {
    fn from(value: i32) -> Self {
        match value {
            5 => Platform::StandaloneWinPlayer,
            4 => Platform::StandaloneOSX,
            17 => Platform::StandaloneLinux64,
            20 => Platform::WebGL,
            _ => Platform::Unknown,
        }
    }
}

/// Parsed Unity SerializedFile.
#[allow(dead_code)]
pub struct SerializedFile {
    pub version: u64,
    pub data_offset: u64,
    pub unity_version: String,
    pub platform: Platform,
    pub types: Vec<SerializedType>,
    pub objects: Vec<ObjectInfo>,
    reader: DataReader,
}

impl SerializedFile {
    /// Parse a Unity assets file.
    pub fn parse<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let mut file = File::open(path.as_ref())?;
        let header = FileHeader::parse(&mut file)?;

        if header.file_type != FileType::AssetsFile {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Not a valid Unity assets file",
            ));
        }

        let mut reader = DataReader::new(file, header.big_endian)?;

        // Skip past the header we already read
        // Position should be right after header
        // Skip 12 ints (48 bytes) that were read but not used
        for _ in 0..12 {
            reader.read_i32()?;
        }

        let unity_version = if header.version >= 7 {
            reader.read_string_to_null()?
        } else {
            String::new()
        };

        let platform = if header.version >= 8 {
            Platform::from(reader.read_i32()?)
        } else {
            Platform::Unknown
        };

        let enable_type_tree = if header.version >= 13 {
            reader.read_bool()?
        } else {
            false
        };

        // Read types
        let type_count = reader.read_i32()? as usize;
        let types =
            SerializedType::parse_types(&mut reader, header.version, enable_type_tree, type_count)?;

        let big_id_enabled = if header.version >= 7 && header.version < 14 {
            reader.read_i32()? != 0
        } else {
            false
        };

        // Read objects
        let object_count = reader.read_i32()? as usize;
        let mut objects = Vec::with_capacity(object_count);

        for _ in 0..object_count {
            let obj = ObjectInfo::parse(
                &mut reader,
                header.version,
                header.data_offset,
                big_id_enabled,
                &types,
            )?;
            objects.push(obj);
        }

        // Skip remaining metadata (scripts, externals, etc.)
        // We only need the objects for extraction

        Ok(Self {
            version: header.version,
            data_offset: header.data_offset,
            unity_version,
            platform,
            types,
            objects,
            reader,
        })
    }

    /// Get mutable reference to reader for reading object data.
    pub fn reader_mut(&mut self) -> &mut DataReader {
        &mut self.reader
    }
}

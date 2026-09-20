//! Unity asset file header parsing.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};

/// Type of Unity file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    ResourceFile,
    AssetsFile,
}

/// Unity asset file header.
#[derive(Debug)]
pub struct FileHeader {
    pub metadata_size: u64,
    pub file_size: u64,
    pub version: u64,
    pub data_offset: u64,
    pub size: u64,
    pub big_endian: bool,
    pub file_type: FileType,
}

impl FileHeader {
    /// Parse file header from a Unity asset file.
    pub fn parse(file: &mut File) -> io::Result<Self> {
        let size = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(0))?;

        let mut buf4 = [0u8; 4];

        // Read initial header (big endian)
        file.read_exact(&mut buf4)?;
        let metadata_size = u32::from_be_bytes(buf4) as u64;

        file.read_exact(&mut buf4)?;
        let file_size_initial = u32::from_be_bytes(buf4) as u64;

        file.read_exact(&mut buf4)?;
        let version = u32::from_be_bytes(buf4) as u64;

        file.read_exact(&mut buf4)?;
        let data_offset_initial = u32::from_be_bytes(buf4) as u64;

        let (metadata_size, file_size, data_offset, big_endian) = if version >= 22 {
            // Version 22+ has extended header
            let mut buf1 = [0u8; 1];
            file.read_exact(&mut buf1)?;
            let big_endian = buf1[0] != 0;

            // Skip 3 reserved bytes
            let mut reserved = [0u8; 3];
            file.read_exact(&mut reserved)?;

            // Re-read with correct sizes
            file.read_exact(&mut buf4)?;
            let metadata_size = u32::from_be_bytes(buf4) as u64;

            let mut buf8 = [0u8; 8];
            file.read_exact(&mut buf8)?;
            let file_size = u64::from_be_bytes(buf8);

            file.read_exact(&mut buf8)?;
            let data_offset = u64::from_be_bytes(buf8);

            // Skip unknown 8 bytes
            file.read_exact(&mut buf8)?;

            (metadata_size, file_size, data_offset, big_endian)
        } else {
            (metadata_size, file_size_initial, data_offset_initial, false)
        };

        // Determine file type
        let file_type = if version > 100
            || file_size > size
            || data_offset > size
            || file_size < metadata_size
            || file_size < data_offset
            || metadata_size > size
            || version > size
        {
            FileType::ResourceFile
        } else {
            FileType::AssetsFile
        };

        Ok(Self {
            metadata_size,
            file_size,
            version,
            data_offset,
            size,
            big_endian,
            file_type,
        })
    }
}

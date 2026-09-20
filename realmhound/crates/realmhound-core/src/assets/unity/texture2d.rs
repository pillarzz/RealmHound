//! Texture2D parsing.

use super::object_reader::ObjectInfo;
use super::reader::DataReader;
use std::fs::File;
use std::io;

/// Spritesheet texture names we want to extract.
pub const SPRITESHEET_NAMES: &[&str] = &[
    "characters",
    "characters_masks",
    "groundTiles",
    "mapObjects",
];

/// Streamed (external `.resS`) texture names we want to extract. These have no
/// embedded pixel data; their bytes are loaded via [`Texture2D::load_streamed_data`].
pub const STREAMED_TEXTURE_NAMES: &[&str] = &["CollectionIcon"];

/// Texture format enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum TextureFormat {
    Unknown = 0,
    RGBA32 = 4,
    ARGB32 = 5,
    RGB24 = 3,
    BGRA32 = 14,
}

impl From<i32> for TextureFormat {
    fn from(value: i32) -> Self {
        match value {
            3 => TextureFormat::RGB24,
            4 => TextureFormat::RGBA32,
            5 => TextureFormat::ARGB32,
            14 => TextureFormat::BGRA32,
            _ => TextureFormat::Unknown,
        }
    }
}

/// Parsed Texture2D from Unity.
#[derive(Debug)]
pub struct Texture2D {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub format: TextureFormat,
    pub image_data: Vec<u8>,
    /// Streaming path (for externally stored textures)
    pub stream_path: String,
    pub stream_offset: u64,
    pub stream_size: u64,
}

impl Texture2D {
    /// Parse a Texture2D from the reader at the object's position.
    pub fn parse(reader: &mut DataReader, obj: &ObjectInfo) -> io::Result<Self> {
        reader.set_position(obj.byte_start)?;

        let name = reader.read_aligned_string()?;

        // Version-dependent fields (assuming Unity 2020+)
        reader.read_bool()?; // m_IsAlphaChannelOptional
        reader.align_stream()?;

        let width = reader.read_i32()?;
        let height = reader.read_i32()?;
        reader.read_i32()?; // m_CompleteImageSize
        reader.read_i32()?; // m_MipsStripped

        let format = TextureFormat::from(reader.read_i32()?);

        reader.read_i32()?; // m_MipCount
        reader.read_bool()?; // m_IsReadable
        reader.read_bool()?; // m_IsPreProcessed
        reader.read_bool()?; // m_IgnoreMasterTextureLimit
        reader.read_bool()?; // m_StreamingMipmaps
        reader.align_stream()?;

        reader.read_i32()?; // m_StreamingMipmapsPriority
        reader.read_i32()?; // m_ImageCount
        reader.read_i32()?; // m_TextureDimension

        // GLTextureSettings
        reader.read_i32()?; // m_FilterMode
        reader.read_i32()?; // m_Aniso
        reader.read_f32()?; // m_MipBias
        reader.read_i32()?; // m_WrapMode
        reader.read_i32()?; // m_WrapV
        reader.read_i32()?; // m_WrapW

        reader.read_i32()?; // m_LightmapFormat
        reader.read_i32()?; // m_ColorSpace

        // Platform blob
        let _platform_blob = reader.read_byte_array()?;
        reader.align_stream()?;

        let image_data_size = reader.read_i32()?;
        let image_data = if image_data_size > 0 {
            reader.read_bytes(image_data_size as usize)?
        } else {
            Vec::new()
        };

        // Streaming info
        let stream_offset = reader.read_i64()? as u64;
        let stream_size = reader.read_u32()? as u64;
        let stream_path = reader.read_aligned_string()?;

        Ok(Self {
            name,
            width,
            height,
            format,
            image_data,
            stream_path,
            stream_offset,
            stream_size,
        })
    }

    /// Check if this is a spritesheet we want to extract.
    pub fn is_spritesheet(&self) -> bool {
        SPRITESHEET_NAMES.contains(&self.name.as_str())
    }

    /// Whether this texture's pixels live in an external streaming resource
    /// (`.resS`/`.resource`) rather than embedded in the serialized file.
    pub fn is_streamed(&self) -> bool {
        self.image_data.is_empty() && self.stream_size > 0 && !self.stream_path.is_empty()
    }

    /// Load streamed pixel data from the external resource file into
    /// `image_data`, so `to_png()` can encode it like an embedded texture.
    ///
    /// `base_dir` is the directory containing the serialized file
    /// (e.g. the directory holding `resources.assets`). The Unity
    /// `StreamingInfo.path` may be a bare filename or an `archive:/CAB-x/NAME`
    /// path; only the final path component is used, resolved relative to
    /// `base_dir`.
    pub fn load_streamed_data(&mut self, base_dir: &std::path::Path) -> io::Result<()> {
        use std::io::{Read, Seek, SeekFrom};

        let file_name = self
            .stream_path
            .rsplit(['/', '\\'])
            .next()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Empty streaming path for texture {}", self.name),
                )
            })?;

        let res_path = base_dir.join(file_name);
        let mut file = File::open(&res_path).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("Failed to open streaming resource {:?}: {}", res_path, e),
            )
        })?;

        let len = file.metadata()?.len();
        let end = self
            .stream_offset
            .checked_add(self.stream_size)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Stream range overflow for texture {}", self.name),
                )
            })?;
        if end > len {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!(
                    "Stream range {}..{} out of bounds (file len {}) for texture {}",
                    self.stream_offset, end, len, self.name
                ),
            ));
        }

        file.seek(SeekFrom::Start(self.stream_offset))?;
        let mut data = vec![0u8; self.stream_size as usize];
        file.read_exact(&mut data)?;
        self.image_data = data;
        Ok(())
    }

    /// Convert RGBA image data to PNG bytes.
    /// The image data comes in bottom-to-top order, so we need to flip it.
    pub fn to_png(&self) -> io::Result<Vec<u8>> {
        if self.image_data.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Texture {} has no embedded data (streaming: {})",
                    self.name, self.stream_path
                ),
            ));
        }

        // Convert to RGBA if needed
        let rgba_data = match self.format {
            TextureFormat::RGBA32 => self.image_data.clone(),
            TextureFormat::ARGB32 => {
                // ARGB -> RGBA
                let mut rgba = Vec::with_capacity(self.image_data.len());
                for chunk in self.image_data.chunks_exact(4) {
                    rgba.push(chunk[1]); // R
                    rgba.push(chunk[2]); // G
                    rgba.push(chunk[3]); // B
                    rgba.push(chunk[0]); // A
                }
                rgba
            }
            TextureFormat::BGRA32 => {
                // BGRA -> RGBA
                let mut rgba = Vec::with_capacity(self.image_data.len());
                for chunk in self.image_data.chunks_exact(4) {
                    rgba.push(chunk[2]); // R
                    rgba.push(chunk[1]); // G
                    rgba.push(chunk[0]); // B
                    rgba.push(chunk[3]); // A
                }
                rgba
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("Unsupported texture format: {:?}", self.format),
                ));
            }
        };

        // Flip image vertically (Unity stores bottom-to-top)
        let row_bytes = (self.width * 4) as usize;
        let mut flipped = vec![0u8; rgba_data.len()];
        for y in 0..self.height as usize {
            let src_start = y * row_bytes;
            let dst_start = (self.height as usize - 1 - y) * row_bytes;
            flipped[dst_start..dst_start + row_bytes]
                .copy_from_slice(&rgba_data[src_start..src_start + row_bytes]);
        }

        // Encode as PNG
        let mut png_data = Vec::new();
        {
            let mut encoder =
                png::Encoder::new(&mut png_data, self.width as u32, self.height as u32);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder
                .write_header()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            writer
                .write_image_data(&flipped)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        }

        Ok(png_data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn streamed(name: &str, path: &str, offset: u64, size: u64) -> Texture2D {
        Texture2D {
            name: name.to_string(),
            width: 4,
            height: 1,
            format: TextureFormat::RGBA32,
            image_data: Vec::new(),
            stream_path: path.to_string(),
            stream_offset: offset,
            stream_size: size,
        }
    }

    #[test]
    fn is_streamed_detects_external_pixels() {
        assert!(streamed("CollectionIcon", "resources.assets.resS", 0, 16).is_streamed());
        // Empty path or zero size => not streamed.
        assert!(!streamed("X", "", 0, 16).is_streamed());
        assert!(!streamed("X", "some.resS", 0, 0).is_streamed());
    }

    #[test]
    fn load_streamed_resolves_bare_filename_and_reads_range() {
        let dir = std::env::temp_dir().join(format!("rh_tex_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let res = dir.join("resources.assets.resS");
        let bytes: Vec<u8> = (0u8..32).collect();
        std::fs::File::create(&res)
            .unwrap()
            .write_all(&bytes)
            .unwrap();

        // archive-style path: only the final component is used.
        let mut tex = streamed(
            "CollectionIcon",
            "archive:/CAB-abc/resources.assets.resS",
            8,
            4,
        );
        tex.load_streamed_data(&dir).unwrap();
        assert_eq!(tex.image_data, vec![8, 9, 10, 11]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_streamed_rejects_out_of_range() {
        let dir = std::env::temp_dir().join(format!("rh_tex_oob_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let res = dir.join("resources.assets.resS");
        std::fs::File::create(&res)
            .unwrap()
            .write_all(&[0u8; 16])
            .unwrap();

        let mut tex = streamed("CollectionIcon", "resources.assets.resS", 8, 16);
        let err = tex.load_streamed_data(&dir).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_streamed_missing_file_errors() {
        let dir = std::env::temp_dir().join(format!("rh_tex_missing_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut tex = streamed("CollectionIcon", "resources.assets.resS", 0, 4);
        assert!(tex.load_streamed_data(&dir).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}

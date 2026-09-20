//! Pic packet implementation.
//!
//! A packet which contains a bitmap image.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Pic packet (ID 83) - Incoming
#[derive(Debug, Clone)]
pub struct PicPacket {
    /// The width of the image.
    pub width: i32,
    /// The height of the image.
    pub height: i32,
    /// The RGBA bitmap data of the image (`width * height * 4` bytes).
    pub bitmap_data: Vec<u8>,
}

impl RotmgPacket for PicPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let width = reader.read_i32()?;
        let height = reader.read_i32()?;

        // Compute the bitmap byte length in i64 to avoid overflow, and reject
        // implausible dimensions before allocating.
        if width < 0 || height < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Negative image dimensions: {}x{}", width, height),
            ));
        }
        let byte_len = (width as i64) * (height as i64) * 4;
        if byte_len > reader.remaining() as i64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Bitmap length {} exceeds remaining {}",
                    byte_len,
                    reader.remaining()
                ),
            ));
        }
        let bitmap_data = reader.read_bytes(byte_len as usize)?;

        Ok(Self {
            width,
            height,
            bitmap_data,
        })
    }

    fn description(&self) -> String {
        format!(
            "Pic: {}x{} ({} bytes)",
            self.width,
            self.height,
            self.bitmap_data.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&2i32.to_be_bytes()); // width
        data.extend_from_slice(&1i32.to_be_bytes()); // height
        data.extend_from_slice(&[0u8; 8]); // 2*1*4 bytes

        let mut reader = PacketReader::new(&data);
        let packet = PicPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.width, 2);
        assert_eq!(packet.height, 1);
        assert_eq!(packet.bitmap_data.len(), 8);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_oversized_rejected() {
        let mut data = Vec::new();
        data.extend_from_slice(&1000i32.to_be_bytes()); // width
        data.extend_from_slice(&1000i32.to_be_bytes()); // height (4M bytes, none present)
        let mut reader = PacketReader::new(&data);
        assert!(PicPacket::deserialize(&mut reader).is_err());
    }
}

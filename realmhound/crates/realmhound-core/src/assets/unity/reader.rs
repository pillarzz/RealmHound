//! Binary data reader with endian support.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};

/// Binary data reader supporting both big and little endian.
pub struct DataReader {
    file: File,
    big_endian: bool,
    length: u64,
}

impl DataReader {
    /// Create a new reader from a file.
    pub fn new(mut file: File, big_endian: bool) -> io::Result<Self> {
        let length = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(0))?;
        Ok(Self {
            file,
            big_endian,
            length,
        })
    }

    /// Get current position in the file.
    pub fn position(&mut self) -> io::Result<u64> {
        self.file.stream_position()
    }

    /// Set position in the file.
    pub fn set_position(&mut self, pos: u64) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(pos))?;
        Ok(())
    }

    /// Get total file length.
    pub fn length(&self) -> u64 {
        self.length
    }

    /// Read a single byte.
    pub fn read_byte(&mut self) -> io::Result<u8> {
        let mut buf = [0u8; 1];
        self.file.read_exact(&mut buf)?;
        Ok(buf[0])
    }

    /// Read N bytes.
    pub fn read_bytes(&mut self, count: usize) -> io::Result<Vec<u8>> {
        let mut buf = vec![0u8; count];
        self.file.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Read a boolean (single byte).
    pub fn read_bool(&mut self) -> io::Result<bool> {
        Ok(self.read_byte()? != 0)
    }

    /// Read i16 with endian support.
    pub fn read_i16(&mut self) -> io::Result<i16> {
        let mut buf = [0u8; 2];
        self.file.read_exact(&mut buf)?;
        Ok(if self.big_endian {
            i16::from_be_bytes(buf)
        } else {
            i16::from_le_bytes(buf)
        })
    }

    /// Read u16 with endian support.
    pub fn read_u16(&mut self) -> io::Result<u16> {
        let mut buf = [0u8; 2];
        self.file.read_exact(&mut buf)?;
        Ok(if self.big_endian {
            u16::from_be_bytes(buf)
        } else {
            u16::from_le_bytes(buf)
        })
    }

    /// Read i32 with endian support.
    pub fn read_i32(&mut self) -> io::Result<i32> {
        let mut buf = [0u8; 4];
        self.file.read_exact(&mut buf)?;
        Ok(if self.big_endian {
            i32::from_be_bytes(buf)
        } else {
            i32::from_le_bytes(buf)
        })
    }

    /// Read u32 with endian support.
    pub fn read_u32(&mut self) -> io::Result<u32> {
        let mut buf = [0u8; 4];
        self.file.read_exact(&mut buf)?;
        Ok(if self.big_endian {
            u32::from_be_bytes(buf)
        } else {
            u32::from_le_bytes(buf)
        })
    }

    /// Read i64 with endian support.
    pub fn read_i64(&mut self) -> io::Result<i64> {
        let mut buf = [0u8; 8];
        self.file.read_exact(&mut buf)?;
        Ok(if self.big_endian {
            i64::from_be_bytes(buf)
        } else {
            i64::from_le_bytes(buf)
        })
    }

    /// Read u64 with endian support.
    pub fn read_u64(&mut self) -> io::Result<u64> {
        let mut buf = [0u8; 8];
        self.file.read_exact(&mut buf)?;
        Ok(if self.big_endian {
            u64::from_be_bytes(buf)
        } else {
            u64::from_le_bytes(buf)
        })
    }

    /// Read f32 with endian support.
    pub fn read_f32(&mut self) -> io::Result<f32> {
        Ok(f32::from_bits(self.read_u32()?))
    }

    /// Align stream to 4-byte boundary.
    pub fn align_stream(&mut self) -> io::Result<()> {
        let pos = self.position()?;
        let skip = (4 - pos % 4) % 4;
        if skip > 0 {
            self.file.seek(SeekFrom::Current(skip as i64))?;
        }
        Ok(())
    }

    /// Read null-terminated string.
    pub fn read_string_to_null(&mut self) -> io::Result<String> {
        let mut bytes = Vec::new();
        loop {
            let b = self.read_byte()?;
            if b == 0 {
                break;
            }
            bytes.push(b);
        }
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }

    /// Read aligned string (length-prefixed with 4-byte alignment).
    pub fn read_aligned_string(&mut self) -> io::Result<String> {
        let length = self.read_i32()? as usize;
        let remaining = self.length - self.position()?;

        if length == 0 || length > remaining as usize {
            return Ok(String::new());
        }

        let bytes = self.read_bytes(length)?;
        self.align_stream()?;
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }

    /// Read a byte array with i32 length prefix.
    pub fn read_byte_array(&mut self) -> io::Result<Vec<u8>> {
        let length = self.read_i32()? as usize;
        self.read_bytes(length)
    }
}

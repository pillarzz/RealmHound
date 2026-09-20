//! Packet reader for deserializing RotMG packet data.
//!
//! This module provides utilities for reading primitive types
//! from packet byte buffers in the correct byte order.

use std::io::{self, Read};

/// Maximum reasonable array length in a packet (64KB elements).
/// This prevents capacity overflow from corrupted data.
const MAX_ARRAY_LENGTH: usize = 65536;

/// A reader for deserializing packet data.
///
/// Provides methods for reading various data types in the format
/// used by RotMG packets (big-endian byte order).
pub struct PacketReader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> PacketReader<'a> {
    /// Create a new packet reader from a byte slice.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    /// Get the current read position.
    pub fn position(&self) -> usize {
        self.position
    }

    /// Get the number of bytes remaining.
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.position)
    }

    /// Check if the reader has reached the end.
    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// Skip n bytes.
    pub fn skip(&mut self, n: usize) -> io::Result<()> {
        if self.remaining() < n {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "not enough bytes to skip",
            ));
        }
        self.position += n;
        Ok(())
    }

    /// Skip all remaining bytes (marks packet as fully consumed).
    /// Use this when packet format has changed but core data was parsed.
    pub fn skip_remaining(&mut self) {
        self.position = self.data.len();
    }

    /// Read a single byte.
    pub fn read_byte(&mut self) -> io::Result<u8> {
        if self.remaining() < 1 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "not enough bytes for u8",
            ));
        }
        let value = self.data[self.position];
        self.position += 1;
        Ok(value)
    }

    /// Read a boolean (1 byte).
    pub fn read_bool(&mut self) -> io::Result<bool> {
        Ok(self.read_byte()? != 0)
    }

    /// Read a signed 16-bit integer (big-endian).
    pub fn read_i16(&mut self) -> io::Result<i16> {
        if self.remaining() < 2 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "not enough bytes for i16",
            ));
        }
        let bytes = [self.data[self.position], self.data[self.position + 1]];
        self.position += 2;
        Ok(i16::from_be_bytes(bytes))
    }

    /// Read an unsigned 16-bit integer (big-endian).
    pub fn read_u16(&mut self) -> io::Result<u16> {
        if self.remaining() < 2 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "not enough bytes for u16",
            ));
        }
        let bytes = [self.data[self.position], self.data[self.position + 1]];
        self.position += 2;
        Ok(u16::from_be_bytes(bytes))
    }

    /// Read a signed 32-bit integer (big-endian).
    pub fn read_i32(&mut self) -> io::Result<i32> {
        if self.remaining() < 4 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "not enough bytes for i32",
            ));
        }
        let bytes = [
            self.data[self.position],
            self.data[self.position + 1],
            self.data[self.position + 2],
            self.data[self.position + 3],
        ];
        self.position += 4;
        Ok(i32::from_be_bytes(bytes))
    }

    /// Read an unsigned 32-bit integer (big-endian).
    pub fn read_u32(&mut self) -> io::Result<u32> {
        if self.remaining() < 4 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "not enough bytes for u32",
            ));
        }
        let bytes = [
            self.data[self.position],
            self.data[self.position + 1],
            self.data[self.position + 2],
            self.data[self.position + 3],
        ];
        self.position += 4;
        Ok(u32::from_be_bytes(bytes))
    }

    /// Read a 32-bit float (big-endian).
    pub fn read_f32(&mut self) -> io::Result<f32> {
        let bits = self.read_u32()?;
        Ok(f32::from_bits(bits))
    }

    /// Read a UTF-8 string with a 16-bit length prefix.
    /// Uses lossy conversion to handle non-UTF8 bytes (replaces invalid sequences with U+FFFD).
    pub fn read_string(&mut self) -> io::Result<String> {
        let len = self.read_u16()? as usize;
        if self.remaining() < len {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "not enough bytes for string",
            ));
        }
        let bytes = &self.data[self.position..self.position + len];
        self.position += len;
        // Use lossy conversion to handle potential non-UTF8 data from game
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }

    /// Read a UTF-8 string with a 32-bit length prefix.
    /// Uses lossy conversion to handle non-UTF8 bytes.
    pub fn read_string32(&mut self) -> io::Result<String> {
        let len = self.read_u32()? as usize;
        if self.remaining() < len {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "not enough bytes for string32",
            ));
        }
        let bytes = &self.data[self.position..self.position + len];
        self.position += len;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }

    /// Read raw bytes.
    pub fn read_bytes(&mut self, len: usize) -> io::Result<Vec<u8>> {
        if self.remaining() < len {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "not enough bytes",
            ));
        }
        let bytes = self.data[self.position..self.position + len].to_vec();
        self.position += len;
        Ok(bytes)
    }

    /// Read remaining bytes.
    pub fn read_remaining(&mut self) -> Vec<u8> {
        let bytes = self.data[self.position..].to_vec();
        self.position = self.data.len();
        bytes
    }

    /// Peek at the next byte without consuming it.
    pub fn peek_byte(&self) -> Option<u8> {
        if self.remaining() > 0 {
            Some(self.data[self.position])
        } else {
            None
        }
    }

    /// Read a byte array with a 16-bit length prefix.
    pub fn read_byte_array(&mut self) -> io::Result<Vec<u8>> {
        let len = self.read_u16()? as usize;
        self.read_bytes(len)
    }

    /// Read a compressed integer (variable-length encoding used by RotMG).
    ///
    /// This encoding uses 7 bits per byte for the value, with the MSB
    /// indicating if more bytes follow. Bit 6 of the first byte indicates sign.
    pub fn read_compressed_int(&mut self) -> io::Result<i32> {
        let first_byte = self.read_byte()? as u32;
        let is_negative = (first_byte & 64) != 0;
        let mut shift = 6u32;
        let mut value = (first_byte & 63) as i32;

        let mut current_byte = first_byte;
        while (current_byte & 128) != 0 {
            current_byte = self.read_byte()? as u32;
            // Prevent overflow - compressed int should fit in 32 bits (max 5 bytes)
            if shift >= 32 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Compressed int overflow - corrupted data",
                ));
            }
            value |= ((current_byte & 127) as i32) << shift;
            shift += 7;
        }

        if is_negative {
            Ok(-value)
        } else {
            Ok(value)
        }
    }

    /// Read and validate an array length from a compressed int.
    /// Returns an error if the length exceeds MAX_ARRAY_LENGTH or is negative.
    pub fn read_array_length(&mut self) -> io::Result<usize> {
        let len = self.read_compressed_int()?;
        if len < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Negative array length: {}", len),
            ));
        }
        let len = len as usize;
        if len > MAX_ARRAY_LENGTH {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Array length {} exceeds maximum {}", len, MAX_ARRAY_LENGTH),
            ));
        }
        Ok(len)
    }

    /// Check if the buffer has been fully parsed.
    pub fn is_fully_parsed(&self) -> bool {
        self.position == self.data.len()
    }
}

impl<'a> Read for PacketReader<'a> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let to_read = std::cmp::min(buf.len(), self.remaining());
        buf[..to_read].copy_from_slice(&self.data[self.position..self.position + to_read]);
        self.position += to_read;
        Ok(to_read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_primitives() {
        let data = [0x00, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x03];
        let mut reader = PacketReader::new(&data);

        assert_eq!(reader.read_byte().unwrap(), 0);
        assert_eq!(reader.read_byte().unwrap(), 1);
        assert_eq!(reader.read_u16().unwrap(), 2);
        assert_eq!(reader.read_u32().unwrap(), 3);
        assert!(reader.is_empty());
    }

    #[test]
    fn test_read_string() {
        let data = [0x00, 0x05, b'H', b'e', b'l', b'l', b'o'];
        let mut reader = PacketReader::new(&data);

        assert_eq!(reader.read_string().unwrap(), "Hello");
        assert!(reader.is_empty());
    }

    #[test]
    fn test_read_compressed_int_positive() {
        // Simple positive number: 42 fits in 6 bits
        let data = [42];
        let mut reader = PacketReader::new(&data);
        assert_eq!(reader.read_compressed_int().unwrap(), 42);
    }

    #[test]
    fn test_read_compressed_int_negative() {
        // Negative number: -42 (bit 6 set for negative)
        let data = [42 | 64]; // 0b01101010 = 106
        let mut reader = PacketReader::new(&data);
        assert_eq!(reader.read_compressed_int().unwrap(), -42);
    }

    #[test]
    fn test_read_compressed_int_multi_byte() {
        // Larger number requiring multiple bytes
        // 200 = 0b11001000, needs 2 bytes in compressed format
        // First byte: continuation bit (128) + lower 6 bits = 128 | 8 = 136
        // Second byte: remaining bits = 200 >> 6 = 3
        let data = [136, 3];
        let mut reader = PacketReader::new(&data);
        assert_eq!(reader.read_compressed_int().unwrap(), 200);
    }

    #[test]
    fn test_read_byte_array() {
        let data = [0x00, 0x03, 0xAA, 0xBB, 0xCC];
        let mut reader = PacketReader::new(&data);
        assert_eq!(reader.read_byte_array().unwrap(), vec![0xAA, 0xBB, 0xCC]);
    }
}

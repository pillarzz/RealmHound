//! Raw packet capture recorder and reader.
//!
//! Records the exact [`RawPacket`] stream that feeds the processing pipeline so
//! a session can be replayed offline (feed the read packets back through the
//! same reassembler + parser). Used as the input source for the Combat History
//! feasibility spike and as a fixture format for combat-reconstruction tests.
//!
//! File format (`.rhcap`), little-endian:
//! - Magic: `b"RHCAP\x01"` (6 bytes)
//! - Repeated frames: `[ts_nanos: i64][len: u32][data: len bytes]`
//!
//! `ts_nanos` is nanoseconds since the Unix epoch. `data` is the raw captured
//! bytes (link-layer through TCP payload), identical to what capture delivers.

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::RawPacket;

const MAGIC: &[u8; 6] = b"RHCAP\x01";

/// Directory where session captures are written:
/// `%LOCALAPPDATA%\RealmHound\captures\`.
pub fn captures_dir() -> PathBuf {
    let base = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("RealmHound")
        .join("captures");
    let _ = fs::create_dir_all(&base);
    base
}

/// Streaming writer for a `.rhcap` capture file.
pub struct CaptureWriter {
    inner: BufWriter<File>,
    count: u64,
}

impl CaptureWriter {
    /// Create a capture file at `path`, writing the magic header.
    pub fn create(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut inner = BufWriter::new(File::create(path)?);
        inner.write_all(MAGIC)?;
        Ok(Self { inner, count: 0 })
    }

    /// Append one raw packet frame.
    pub fn write(&mut self, packet: &RawPacket) -> io::Result<()> {
        let ts_nanos = packet.timestamp.timestamp_nanos_opt().unwrap_or(0);
        self.inner.write_all(&ts_nanos.to_le_bytes())?;
        let len = packet.data.len() as u32;
        self.inner.write_all(&len.to_le_bytes())?;
        self.inner.write_all(&packet.data)?;
        self.count += 1;
        Ok(())
    }

    /// Number of frames written so far.
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Flush buffered bytes to disk.
    pub fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Drop for CaptureWriter {
    fn drop(&mut self) {
        let _ = self.inner.flush();
    }
}

/// Read every frame from a `.rhcap` capture file into raw packets.
pub fn read_capture(path: &Path) -> io::Result<Vec<RawPacket>> {
    let mut reader = BufReader::new(File::open(path)?);

    let mut magic = [0u8; 6];
    reader.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a RealmHound capture file (bad magic)",
        ));
    }

    let mut packets = Vec::new();
    loop {
        let mut ts_buf = [0u8; 8];
        match reader.read_exact(&mut ts_buf) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        }
        let ts_nanos = i64::from_le_bytes(ts_buf);

        let mut len_buf = [0u8; 4];
        reader.read_exact(&mut len_buf)?;
        let len = u32::from_le_bytes(len_buf) as usize;

        let mut data = vec![0u8; len];
        reader.read_exact(&mut data)?;

        let timestamp: DateTime<Utc> = DateTime::from_timestamp_nanos(ts_nanos);
        packets.push(RawPacket {
            timestamp,
            #[cfg(feature = "latency-diagnostics")]
            enqueued_at: None,
            data,
        });
    }
    Ok(packets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_packets() {
        let dir = std::env::temp_dir().join("rhcap_test");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("sample.rhcap");

        let original = vec![
            RawPacket {
                timestamp: Utc::now(),
                #[cfg(feature = "latency-diagnostics")]
                enqueued_at: None,
                data: vec![1, 2, 3, 4],
            },
            RawPacket {
                timestamp: Utc::now(),
                #[cfg(feature = "latency-diagnostics")]
                enqueued_at: None,
                data: vec![],
            },
            RawPacket {
                timestamp: Utc::now(),
                #[cfg(feature = "latency-diagnostics")]
                enqueued_at: None,
                data: vec![9; 100],
            },
        ];

        {
            let mut w = CaptureWriter::create(&path).unwrap();
            for p in &original {
                w.write(p).unwrap();
            }
            assert_eq!(w.count(), 3);
        }

        let read = read_capture(&path).unwrap();
        assert_eq!(read.len(), original.len());
        for (a, b) in original.iter().zip(read.iter()) {
            assert_eq!(a.data, b.data);
        }
    }

    #[test]
    fn rejects_bad_magic() {
        let dir = std::env::temp_dir().join("rhcap_test");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("bad.rhcap");
        fs::write(&path, b"NOPE!!more").unwrap();
        assert!(read_capture(&path).is_err());
    }
}

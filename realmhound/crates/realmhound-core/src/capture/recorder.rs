//! Raw packet capture recorder and reader.
//!
//! Records the exact [`RawPacket`] stream that feeds the processing pipeline so
//! a session can be replayed offline (feed the read packets back through the
//! same reassembler + parser). Used as the input source for the Combat History
//! feasibility spike and as a fixture format for combat-reconstruction tests.
//!
//! File format (`.rhcap`), little-endian:
//! - Magic: `b"RHCAP\x02"` (6 bytes)
//! - Repeated frames: `[ts_nanos: i64][format: u8][len: u32][data: len bytes]`
//! - Format identifiers: 0 = Ethernet, 1 = raw IP, 2 = DLT_NULL, 3 = DLT_LOOP.
//!
//! Version 1 (`b"RHCAP\x01"`) has no format byte and is read as Ethernet.
//!
//! `ts_nanos` is nanoseconds since the Unix epoch. `data` is the raw captured
//! bytes (link-layer through TCP payload), identical to what capture delivers.

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::{PacketFormat, RawPacket};

const MAGIC: &[u8; 6] = b"RHCAP\x02";

/// Directory where session captures are written: `captures/` under the
/// per-user local data directory (see [`crate::storage::StorageRoot`]).
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
        self.inner.write_all(&[packet.packet_format as u8])?;
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
    if &magic[..5] != b"RHCAP" || !matches!(magic[5], 1 | 2) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a RealmHound capture file (bad magic)",
        ));
    }

    let mut packets = Vec::new();
    loop {
        let mut ts_buf = [0u8; 8];
        if reader.read(&mut ts_buf[..1])? == 0 {
            break;
        }
        reader.read_exact(&mut ts_buf[1..])?;
        let ts_nanos = i64::from_le_bytes(ts_buf);

        let packet_format = if magic[5] == 1 {
            PacketFormat::Ethernet
        } else {
            let mut format = [0u8; 1];
            reader.read_exact(&mut format)?;
            match format[0] {
                0 => PacketFormat::Ethernet,
                1 => PacketFormat::RawIp,
                2 => PacketFormat::Loopback,
                3 => PacketFormat::LoopbackNetwork,
                value => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("unsupported capture packet format {value}"),
                    ));
                }
            }
        };

        let mut len_buf = [0u8; 4];
        reader.read_exact(&mut len_buf)?;
        let len = u32::from_le_bytes(len_buf) as usize;

        let mut data = vec![0u8; len];
        reader.read_exact(&mut data)?;

        let timestamp: DateTime<Utc> = DateTime::from_timestamp_nanos(ts_nanos);
        packets.push(RawPacket {
            timestamp,
            packet_format,
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
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.rhcap");

        let original = vec![
            RawPacket {
                timestamp: Utc::now(),
                packet_format: PacketFormat::Ethernet,
                #[cfg(feature = "latency-diagnostics")]
                enqueued_at: None,
                data: vec![1, 2, 3, 4],
            },
            RawPacket {
                timestamp: Utc::now(),
                packet_format: PacketFormat::RawIp,
                #[cfg(feature = "latency-diagnostics")]
                enqueued_at: None,
                data: vec![],
            },
            RawPacket {
                timestamp: Utc::now(),
                packet_format: PacketFormat::Loopback,
                #[cfg(feature = "latency-diagnostics")]
                enqueued_at: None,
                data: vec![9; 100],
            },
            RawPacket {
                timestamp: Utc::now(),
                packet_format: PacketFormat::LoopbackNetwork,
                #[cfg(feature = "latency-diagnostics")]
                enqueued_at: None,
                data: vec![0; 40],
            },
        ];

        {
            let mut w = CaptureWriter::create(&path).unwrap();
            for p in &original {
                w.write(p).unwrap();
            }
            assert_eq!(w.count(), original.len() as u64);
        }

        let read = read_capture(&path).unwrap();
        assert_eq!(read.len(), original.len());
        for (a, b) in original.iter().zip(read.iter()) {
            assert_eq!(a.data, b.data);
            assert_eq!(a.timestamp, b.timestamp);
            assert_eq!(a.packet_format, b.packet_format);
        }
    }

    #[test]
    fn rejects_bad_magic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.rhcap");
        fs::write(&path, b"NOPE!!more").unwrap();
        assert!(read_capture(&path).is_err());
    }

    #[test]
    fn reads_version_one_as_ethernet() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.rhcap");
        let ts_nanos = 1_700_000_000_123_456_789i64;
        let data = [1, 2, 3, 4];
        let mut file = b"RHCAP\x01".to_vec();
        file.extend_from_slice(&ts_nanos.to_le_bytes());
        file.extend_from_slice(&(data.len() as u32).to_le_bytes());
        file.extend_from_slice(&data);
        fs::write(&path, file).unwrap();

        let packets = read_capture(&path).unwrap();
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].timestamp.timestamp_nanos_opt(), Some(ts_nanos));
        assert_eq!(packets[0].packet_format, PacketFormat::Ethernet);
        assert_eq!(packets[0].data, data);
    }

    #[test]
    fn rejects_unknown_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unknown.rhcap");
        let mut file = MAGIC.to_vec();
        file.extend_from_slice(&0i64.to_le_bytes());
        file.push(255);
        fs::write(&path, file).unwrap();
        assert_eq!(
            read_capture(&path).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn rejects_unknown_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unknown-version.rhcap");
        fs::write(&path, b"RHCAP\x03").unwrap();
        assert_eq!(
            read_capture(&path).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn rejects_truncated_frames() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("truncated.rhcap");
        let mut file = MAGIC.to_vec();
        file.extend_from_slice(&0i64.to_le_bytes());
        file.push(PacketFormat::RawIp as u8);
        file.extend_from_slice(&4u32.to_le_bytes());
        file.extend_from_slice(&[1, 2, 3, 4]);
        for length in MAGIC.len() + 1..file.len() {
            fs::write(&path, &file[..length]).unwrap();
            assert_eq!(
                read_capture(&path).unwrap_err().kind(),
                io::ErrorKind::UnexpectedEof,
                "truncated at byte {length}"
            );
        }
    }
}

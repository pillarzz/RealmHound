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

use std::collections::{HashMap, VecDeque};
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::{PacketFormat, RawPacket};

const MAGIC: &[u8; 6] = b"RHCAP\x02";

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
///
/// The most recent frames are held back instead of being written straight away, so
/// the caller can still drop them: whether a frame carries credentials is only known
/// after its packet is parsed, which happens after the frame was handed over (see
/// [`CaptureWriter::discard_pending_flow`]).
pub struct CaptureWriter {
    inner: BufWriter<File>,
    count: u64,
    /// Outgoing client-to-server frames not yet classified. Frames stay here
    /// across processing batches until the reassembled packet is parsed: a Hello
    /// split over several TCP segments (or several batches) is dropped as a
    /// whole, and a non-Hello packet releases its flow's frames.
    pending: HashMap<FlowKey, VecDeque<RawPacket>>,
    /// Frames dropped because a flow exceeded the held bound. A capture can have
    /// a hole here, but credential-bearing bytes are never written unclassified.
    dropped_frames: u64,
}

/// The server's game port. Client-to-server frames on it are the only ones that
/// can carry a Hello session ticket.
const GAME_SERVER_PORT: u16 = 2050;

/// Maximum unresolved outgoing frames held for one flow. A bounded overflow is
/// dropped (fail closed) rather than flushed unclassified.
const MAX_PENDING_PER_FLOW: usize = 64;

type FlowKey = (Ipv4Addr, u16, Ipv4Addr, u16);

impl CaptureWriter {
    /// Create a capture file at `path`, writing the magic header.
    pub fn create(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut inner = BufWriter::new(File::create(path)?);
        inner.write_all(MAGIC)?;
        Ok(Self {
            inner,
            count: 0,
            pending: HashMap::new(),
            dropped_frames: 0,
        })
    }

    /// Append one raw packet frame. Outgoing client-to-server frames are held
    /// (per flow) until [`Self::release_pending_flow`] confirms they are not part
    /// of a Hello; server-to-client frames are written immediately.
    pub fn write(&mut self, packet: &RawPacket) -> io::Result<()> {
        let Some((sip, sport, dip, dport)) = endpoints(packet) else {
            // Unparsable link-layer frames cannot carry a decoded Hello; write
            // them as before.
            return self.write_frame(packet);
        };
        if dport != GAME_SERVER_PORT {
            return self.write_frame(packet);
        }
        let flow = (sip, sport, dip, dport);
        let queue = self.pending.entry(flow).or_default();
        queue.push_back(packet.clone());
        if queue.len() > MAX_PENDING_PER_FLOW {
            let drop = queue.len() - MAX_PENDING_PER_FLOW;
            queue.drain(..drop);
            self.dropped_frames += drop as u64;
            tracing::warn!(
                "[RECORD] Dropped {} unresolved outgoing frame(s) on a flow after \
                 exceeding the {} frame hold bound",
                drop,
                MAX_PENDING_PER_FLOW
            );
        }
        Ok(())
    }

    /// Commit the held frames for the flow `packet` belongs to.
    ///
    /// Called after the reassembled packet was parsed as anything other than an
    /// outgoing Hello. Earlier unresolved packets on the same flow are earlier in
    /// the TCP stream, so they cannot be the Hello either; releasing the whole
    /// queue is safe.
    pub fn release_pending_flow(&mut self, packet: &RawPacket) -> io::Result<()> {
        let Some(key) = endpoints(packet) else {
            return Ok(());
        };
        if let Some(queue) = self.pending.remove(&key) {
            for held in queue {
                self.write_frame(&held)?;
            }
        }
        Ok(())
    }

    /// Drop the held frames that belong to the same TCP flow as `handshake`.
    ///
    /// The client's `Hello` (74) carries the session ticket in its `access_token`.
    /// Captures get shared for debugging, and the packet cipher is keyed by the packet
    /// tick rather than by a secret, so a ticket inside a capture is readable by anyone
    /// holding the file -- these frames never reach the disk.
    ///
    /// Every held frame of that flow *and direction* goes, not only the one that
    /// completed reassembly, because the handshake bytes may be split across TCP
    /// segments. Two kinds of frame are deliberately kept:
    /// - anything the server sent on that connection (its `Hello` reply, and the
    ///   `MapInfo` whose `fp` seeds the damage model): the ticket only travels
    ///   client to server, so those carry nothing worth hiding;
    /// - frames of other connections: dropping them would leave a hole in a stream
    ///   that has nothing to do with the credentials.
    pub fn discard_pending_flow(&mut self, handshake: &RawPacket) {
        let Some(key) = endpoints(handshake) else {
            // Unknown framing: fail closed for everything still held.
            self.dropped_frames += self.pending.values().map(VecDeque::len).sum::<usize>() as u64;
            self.pending.clear();
            return;
        };
        if let Some(queue) = self.pending.remove(&key) {
            self.dropped_frames += queue.len() as u64;
        }
    }

    fn write_frame(&mut self, packet: &RawPacket) -> io::Result<()> {
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

    /// Flush buffered bytes to disk without committing unresolved outgoing frames.
    /// Safe to call at the end of every processing batch; unlike [`Self::finish`]
    /// it keeps the held queues, so a Hello split across batches is still caught.
    pub fn flush_buffered(&mut self) -> io::Result<()> {
        self.inner.flush()
    }

    /// Finish the capture. Unresolved outgoing frames are dropped (fail closed),
    /// never written unclassified. Returns the number of frames lost this way.
    pub fn finish(&mut self) -> io::Result<u64> {
        let unresolved = self.pending.values().map(VecDeque::len).sum::<usize>() as u64;
        if unresolved > 0 {
            tracing::warn!(
                "[RECORD] Dropped {} unresolved outgoing frame(s) at capture stop \
                 (fail closed)",
                unresolved
            );
        }
        self.pending.clear();
        self.dropped_frames += unresolved;
        self.inner.flush()?;
        Ok(self.dropped_frames)
    }
}

impl Drop for CaptureWriter {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

/// `(src_ip, src_port, dst_ip, dst_port)` of a captured frame, if it carries TCP.
fn endpoints(packet: &RawPacket) -> Option<(Ipv4Addr, u16, Ipv4Addr, u16)> {
    let segment = crate::stream::parse_tcp_segment(packet)?;
    Some((
        segment.src_ip,
        segment.src_port,
        segment.dst_ip,
        segment.dst_port,
    ))
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

    /// A minimal IPv4/TCP frame for `port -> 2050`, carrying `payload`.
    fn frame(port: u16, payload: &[u8]) -> RawPacket {
        let mut data = Vec::new();
        etherparse::PacketBuilder::ipv4([192, 0, 2, 1], [198, 51, 100, 2], 64)
            .tcp(port, 2050, 1, 32768)
            .ack(1)
            .psh()
            .write(&mut data, payload)
            .unwrap();
        RawPacket {
            timestamp: Utc::now(),
            packet_format: PacketFormat::RawIp,
            #[cfg(feature = "latency-diagnostics")]
            enqueued_at: None,
            data,
        }
    }

    /// The server's side of the same connection: `2050 -> port`.
    fn reply(port: u16, payload: &[u8]) -> RawPacket {
        let mut data = Vec::new();
        etherparse::PacketBuilder::ipv4([198, 51, 100, 2], [192, 0, 2, 1], 64)
            .tcp(2050, port, 32768, 1)
            .ack(1)
            .psh()
            .write(&mut data, payload)
            .unwrap();
        RawPacket {
            timestamp: Utc::now(),
            packet_format: PacketFormat::RawIp,
            #[cfg(feature = "latency-diagnostics")]
            enqueued_at: None,
            data,
        }
    }

    #[test]
    fn a_discarded_frame_never_reaches_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("redacted.rhcap");
        {
            let mut w = CaptureWriter::create(&path).unwrap();
            let hello = frame(45678, &[1]);
            w.write(&hello).unwrap();
            w.write(&frame(45678, &[2])).unwrap(); // held
            w.write(&frame(45678, &[3])).unwrap(); // held
            w.write(&reply(45678, &[9])).unwrap(); // held, server -> client
                                                   // The caller decided the frame was a handshake after parsing it: it takes
                                                   // out every held frame sent on that connection in that direction, and
                                                   // leaves the server's frames (MapInfo among them) alone.
            w.discard_pending_flow(&hello);
            assert_eq!(w.count(), 1, "the server's reply was written immediately");
            let after = frame(45678, &[4]);
            let other = frame(45679, &[8]);
            w.write(&after).unwrap();
            w.write(&other).unwrap(); // other connection
            w.release_pending_flow(&after).unwrap();
            w.release_pending_flow(&other).unwrap();
            w.finish().unwrap();
            assert_eq!(w.count(), 3);
        }
        let read = read_capture(&path).unwrap();
        assert_eq!(read.len(), 3);
        assert_eq!(endpoints(&read[0]).map(|e| e.1), Some(2050)); // the server's frame
        assert_eq!(endpoints(&read[1]).map(|e| e.1), Some(45678)); // after the Hello
        assert_eq!(endpoints(&read[2]).map(|e| e.1), Some(45679)); // other connection
    }

    #[test]
    fn a_split_hello_is_held_across_batch_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("split.rhcap");
        let mut w = CaptureWriter::create(&path).unwrap();
        let first = frame(45678, &[1]);
        let second = frame(45678, &[2]);
        w.write(&first).unwrap();
        // End of one processing batch: the frame must remain held, not committed.
        w.flush_buffered().unwrap();
        assert_eq!(w.count(), 0);
        w.write(&second).unwrap();
        w.flush_buffered().unwrap();
        assert_eq!(w.count(), 0);
        // The reassembled Hello is classified in the next batch.
        w.discard_pending_flow(&first);
        w.finish().unwrap();
        assert_eq!(w.count(), 0);
        assert!(read_capture(&path).unwrap().is_empty());
    }

    #[test]
    fn a_non_hello_packet_releases_the_holds_for_its_flow() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("release.rhcap");
        let mut w = CaptureWriter::create(&path).unwrap();
        let first = frame(45678, &[1]);
        let second = frame(45678, &[2]);
        w.write(&first).unwrap();
        w.write(&second).unwrap();
        w.flush_buffered().unwrap();
        assert_eq!(w.count(), 0);
        w.release_pending_flow(&second).unwrap();
        assert_eq!(w.count(), 2);
        w.finish().unwrap();
    }

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
            w.finish().unwrap();
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

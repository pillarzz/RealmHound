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
use crate::protocol::{Packet, PacketType};

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
///
/// Outgoing payloads are held until [`Self::classify_packet`] identifies their
/// reassembled TCP byte ranges as non-Hello traffic.
pub struct CaptureWriter {
    inner: BufWriter<File>,
    count: u64,
    flows: HashMap<FlowKey, RecordedFlow>,
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

const MAX_SAFE_BYTES: u32 = 1024 * 1024;

type FlowKey = (Ipv4Addr, u16, Ipv4Addr, u16);

#[derive(Default)]
struct RecordedFlow {
    pending: VecDeque<(RawPacket, ByteRange)>,
    safe: Option<ByteRange>,
}

#[derive(Clone, Copy)]
struct ByteRange {
    start: u32,
    len: u32,
}

impl ByteRange {
    fn contains(self, other: Self) -> bool {
        let offset = other.start.wrapping_sub(self.start);
        offset <= self.len && other.len <= self.len - offset
    }

    fn overlaps(self, other: Self) -> bool {
        self.start.wrapping_sub(other.start) < other.len
            || other.start.wrapping_sub(self.start) < self.len
    }
}

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
            flows: HashMap::new(),
            dropped_frames: 0,
        })
    }

    /// Append a frame, holding outgoing payloads until their full TCP byte range
    /// is classified. Retransmissions must satisfy the same check.
    pub fn write(&mut self, packet: &RawPacket) -> io::Result<()> {
        let Some(segment) = crate::stream::parse_tcp_segment(packet) else {
            return self.write_frame(packet);
        };
        let outgoing = segment.dst_port == GAME_SERVER_PORT;
        let flow = if outgoing {
            (
                segment.src_ip,
                segment.src_port,
                segment.dst_ip,
                segment.dst_port,
            )
        } else {
            (
                segment.dst_ip,
                segment.dst_port,
                segment.src_ip,
                segment.src_port,
            )
        };
        if segment.flags.syn || segment.flags.is_terminating() {
            if let Some(old) = self.flows.remove(&flow) {
                self.dropped_frames += old.pending.len() as u64;
            }
        }
        if !outgoing || segment.payload.is_empty() {
            return self.write_frame(packet);
        }
        let range = ByteRange {
            start: segment.sequence.wrapping_add(u32::from(segment.flags.syn)),
            len: segment.payload.len() as u32,
        };
        let state = self.flows.entry(flow).or_default();
        if state.safe.is_some_and(|safe| safe.contains(range)) {
            return self.write_frame(packet);
        }
        let queue = &mut state.pending;
        queue.push_back((packet.clone(), range));
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

    /// Classify the exact bytes of a reassembled packet. The plaintext packet
    /// header identifies Hello even if its decrypted payload cannot be parsed.
    pub fn classify_packet(&mut self, packet: &Packet) -> io::Result<()> {
        if packet.incoming {
            return Ok(());
        }
        let key = (
            packet.src_ip,
            packet.src_port,
            packet.dst_ip,
            packet.dst_port,
        );
        let state = self.flows.entry(key).or_default();
        let range = ByteRange {
            start: packet.tcp_sequence,
            len: packet.header.size,
        };
        if packet.packet_type() == PacketType::Hello {
            let before = state.pending.len();
            state.pending.retain(|(_, held)| !range.overlaps(*held));
            self.dropped_frames += (before - state.pending.len()) as u64;
            state.safe = None;
            return Ok(());
        }

        let mut safe = match state.safe {
            Some(previous) if previous.start.wrapping_add(previous.len) == range.start => {
                ByteRange {
                    start: previous.start,
                    len: previous.len + range.len,
                }
            }
            _ => range,
        };
        // Keep a bounded recent window: old retransmissions may be dropped, but
        // must never become trusted again after TCP sequence-number wraparound.
        if safe.len > MAX_SAFE_BYTES {
            safe.start = safe.start.wrapping_add(safe.len - MAX_SAFE_BYTES);
            safe.len = MAX_SAFE_BYTES;
        }
        state.safe = Some(safe);
        let mut pending = std::mem::take(&mut state.pending);
        while let Some((held, range)) = pending.pop_front() {
            if safe.contains(range) {
                self.write_frame(&held)?;
            } else {
                self.flows
                    .entry(key)
                    .or_default()
                    .pending
                    .push_back((held, range));
            }
        }
        Ok(())
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
        let unresolved = self
            .flows
            .values()
            .map(|flow| flow.pending.len())
            .sum::<usize>() as u64;
        if unresolved > 0 {
            tracing::warn!(
                "[RECORD] Dropped {} unresolved outgoing frame(s) at capture stop \
                 (fail closed)",
                unresolved
            );
        }
        self.flows.clear();
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
#[cfg(test)]
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
    use crate::crypto::{RC4Cipher, OUTGOING_KEY};
    use crate::stream::{parse_tcp_segment, TcpReassembler};

    /// A minimal IPv4/TCP frame for `port -> 2050`, carrying `payload`.
    fn frame(port: u16, seq: u32, payload: &[u8]) -> RawPacket {
        let mut data = Vec::new();
        etherparse::PacketBuilder::ipv4([192, 0, 2, 1], [198, 51, 100, 2], 64)
            .tcp(port, 2050, seq, 32768)
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

    fn syn(port: u16) -> RawPacket {
        syn_at(port, 0)
    }

    fn syn_at(port: u16, seq: u32) -> RawPacket {
        let mut packet = frame(port, seq, &[]);
        packet.data[33] = 0x02;
        packet
    }

    fn wire_packet(kind: PacketType, payload: &[u8], cipher: &mut RC4Cipher) -> Vec<u8> {
        let mut data = ((5 + payload.len()) as u32).to_be_bytes().to_vec();
        data.push(kind as u8);
        data.extend(cipher.apply_to_vec(payload));
        data
    }

    fn record(writer: &mut CaptureWriter, reassembler: &mut TcpReassembler, raw: &RawPacket) {
        writer.write(raw).unwrap();
        if let Some(segment) = parse_tcp_segment(raw) {
            for packet in reassembler.process_segment(&segment) {
                writer.classify_packet(&packet).unwrap();
            }
        }
        writer.flush_buffered().unwrap();
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
    fn a_retransmitted_hello_never_reaches_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("redacted.rhcap");
        let mut w = CaptureWriter::create(&path).unwrap();
        let mut reassembler = TcpReassembler::new();
        let mut cipher = RC4Cipher::new(OUTGOING_KEY);
        let hello = wire_packet(PacketType::Hello, b"synthetic ticket", &mut cipher);
        let after_seq = 1 + hello.len() as u32;
        let after = wire_packet(PacketType::PlayerText, b"\0\x01x", &mut cipher);
        let hello = frame(45678, 1, &hello);
        let after = frame(45678, after_seq, &after);
        let server = reply(45678, &[9]);
        let other = syn(45679);
        let start = syn(45678);
        for packet in [&start, &hello, &hello, &server, &other, &after] {
            record(&mut w, &mut reassembler, packet);
        }
        w.finish().unwrap();
        let read = read_capture(&path).unwrap();
        let expected = [start, server, other, after];
        assert_eq!(
            read.iter().map(|p| &p.data).collect::<Vec<_>>(),
            expected.iter().map(|p| &p.data).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_split_hello_is_held_across_batch_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("split.rhcap");
        let mut w = CaptureWriter::create(&path).unwrap();
        let mut reassembler = TcpReassembler::new();
        record(&mut w, &mut reassembler, &syn(45678));
        let hello = wire_packet(
            PacketType::Hello,
            &[42; 80],
            &mut RC4Cipher::new(OUTGOING_KEY),
        );
        for (index, chunk) in hello.chunks(3).enumerate() {
            record(
                &mut w,
                &mut reassembler,
                &frame(45678, 1 + index as u32 * 3, chunk),
            );
            assert_eq!(w.count(), 1, "only the SYN may be written");
        }
        w.finish().unwrap();
        assert_eq!(read_capture(&path).unwrap().len(), 1);
    }

    #[test]
    fn an_out_of_order_hello_tail_is_not_mistaken_for_a_safe_packet() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out-of-order.rhcap");
        let mut w = CaptureWriter::create(&path).unwrap();
        let mut reassembler = TcpReassembler::new();
        let mut cipher = RC4Cipher::new(OUTGOING_KEY);
        let mut hello = wire_packet(PacketType::Hello, &[42; 20], &mut cipher);
        let split = hello.len() - 5;
        hello[split..].copy_from_slice(&[0, 0, 0, 5, PacketType::PlayerText as u8]);
        record(&mut w, &mut reassembler, &syn(45678));

        record(
            &mut w,
            &mut reassembler,
            &frame(45678, 1 + split as u32, &hello[split..]),
        );
        assert_eq!(
            w.count(),
            1,
            "the first TCP payload need not start the stream"
        );
        record(&mut w, &mut reassembler, &frame(45678, 1, &hello[..split]));
        let after = wire_packet(PacketType::PlayerText, b"\0\x01x", &mut cipher);
        let after = frame(45678, 1 + hello.len() as u32, &after);
        record(&mut w, &mut reassembler, &after);
        w.finish().unwrap();
        let read = read_capture(&path).unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[1].data, after.data);
    }

    #[test]
    fn a_non_hello_packet_releases_only_its_classified_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("release.rhcap");
        let mut w = CaptureWriter::create(&path).unwrap();
        let mut reassembler = TcpReassembler::new();
        let mut cipher = RC4Cipher::new(OUTGOING_KEY);
        record(&mut w, &mut reassembler, &syn(45678));
        let safe = wire_packet(PacketType::PlayerText, b"\0\x01x", &mut cipher);
        let hello = wire_packet(PacketType::Hello, b"synthetic ticket", &mut cipher);
        let mut combined = safe.clone();
        combined.extend_from_slice(&hello[..3]);
        record(&mut w, &mut reassembler, &frame(45678, 1, &combined));
        assert_eq!(
            w.count(),
            1,
            "a safe prefix must not release the following Hello"
        );
        record(
            &mut w,
            &mut reassembler,
            &frame(45678, 1 + combined.len() as u32, &hello[3..]),
        );
        w.finish().unwrap();
        assert_eq!(read_capture(&path).unwrap().len(), 1);
    }

    #[test]
    fn coalesced_non_hello_packets_release_the_whole_frame() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("coalesced.rhcap");
        let mut w = CaptureWriter::create(&path).unwrap();
        let mut reassembler = TcpReassembler::new();
        let mut cipher = RC4Cipher::new(OUTGOING_KEY);
        record(&mut w, &mut reassembler, &syn(45678));
        let mut data = wire_packet(PacketType::PlayerText, b"\0\x01x", &mut cipher);
        data.extend(wire_packet(PacketType::PlayerText, b"\0\x01y", &mut cipher));
        let safe = frame(45678, 1, &data);
        record(&mut w, &mut reassembler, &safe);
        record(&mut w, &mut reassembler, &safe);
        w.finish().unwrap();
        assert_eq!(read_capture(&path).unwrap().len(), 3);
    }

    #[test]
    fn split_hello_retransmission_is_rejected_across_sequence_wrap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wrap.rhcap");
        let mut w = CaptureWriter::create(&path).unwrap();
        let mut reassembler = TcpReassembler::new();
        let mut cipher = RC4Cipher::new(OUTGOING_KEY);
        let seq = u32::MAX - 7;
        record(
            &mut w,
            &mut reassembler,
            &syn_at(45678, seq.wrapping_sub(1)),
        );
        let hello = wire_packet(PacketType::Hello, b"synthetic ticket", &mut cipher);
        let first = frame(45678, seq, &hello[..7]);
        let second = frame(45678, seq.wrapping_add(7), &hello[7..]);
        let after = wire_packet(PacketType::PlayerText, b"\0\x01x", &mut cipher);
        let after = frame(45678, seq.wrapping_add(hello.len() as u32), &after);
        for packet in [&first, &second, &first, &second, &after] {
            record(&mut w, &mut reassembler, packet);
        }
        w.finish().unwrap();
        let read = read_capture(&path).unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[1].data, after.data);
    }

    #[test]
    fn a_new_syn_invalidates_previously_safe_sequences() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reuse.rhcap");
        let mut w = CaptureWriter::create(&path).unwrap();
        let mut reassembler = TcpReassembler::new();
        let start = syn(45678);
        record(&mut w, &mut reassembler, &start);
        let safe = wire_packet(
            PacketType::PlayerText,
            &[1; 40],
            &mut RC4Cipher::new(OUTGOING_KEY),
        );
        record(&mut w, &mut reassembler, &frame(45678, 1, &safe));
        record(&mut w, &mut reassembler, &start);
        let hello = wire_packet(
            PacketType::Hello,
            b"synthetic ticket",
            &mut RC4Cipher::new(OUTGOING_KEY),
        );
        record(&mut w, &mut reassembler, &frame(45678, 1, &hello));
        w.finish().unwrap();
        assert_eq!(read_capture(&path).unwrap().len(), 3);
    }

    #[test]
    fn an_unresolved_flow_does_not_release_another_flows_hello() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flows.rhcap");
        let mut w = CaptureWriter::create(&path).unwrap();
        let mut reassembler = TcpReassembler::new();
        record(&mut w, &mut reassembler, &syn(45678));
        record(&mut w, &mut reassembler, &syn(45679));
        let hello = wire_packet(
            PacketType::Hello,
            b"synthetic ticket",
            &mut RC4Cipher::new(OUTGOING_KEY),
        );
        record(&mut w, &mut reassembler, &frame(45678, 1, &hello[..3]));
        let safe = wire_packet(
            PacketType::PlayerText,
            b"\0\x01x",
            &mut RC4Cipher::new(OUTGOING_KEY),
        );
        record(&mut w, &mut reassembler, &frame(45679, 1, &safe));
        assert_eq!(w.finish().unwrap(), 1);
        let read = read_capture(&path).unwrap();
        assert_eq!(read.len(), 3);
        assert_eq!(endpoints(&read[2]).unwrap().1, 45679);
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

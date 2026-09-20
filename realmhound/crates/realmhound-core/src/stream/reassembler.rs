//! TCP reassembler for reconstructing packet streams.

use super::connection::{Connection, ConnectionKey};
use super::metrics::CaptureMetrics;
use super::parser::TcpSegment;
use crate::capture::ROTMG_PORT;
use crate::crypto::{RotmgKeys, TickAligner};
use crate::protocol::{Packet, PacketHeader};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Result of attempting to extract a packet from the buffer.
enum ExtractResult {
    /// Successfully extracted a packet
    Packet(Packet),
    /// Need more data to complete the packet
    NeedMoreData,
    /// Invalid header detected (possible stream corruption)
    InvalidHeader,
    /// Cipher not synced yet, waiting for alignment
    Unsynced,
    /// Cipher just synced on this packet; it was consumed without emitting
    /// (the cipher is already positioned past it). Keep extracting.
    SyncedSkip,
}

/// Manages TCP stream reassembly for multiple connections.
pub struct TcpReassembler {
    /// Active connections
    connections: HashMap<ConnectionKey, ConnectionData>,
    /// Connections to silently ignore (secondary game clients).
    ignored_connections: HashSet<ConnectionKey>,
    /// Recently closed connections (FIN/RST). Stray packets arriving shortly
    /// after a close should not create ghost mid-stream connections that evict
    /// real ones.
    recently_closed: HashMap<ConnectionKey, std::time::Instant>,
    /// Maximum number of concurrent connections
    max_connections: usize,
    /// Capture pipeline diagnostics.
    metrics: CaptureMetrics,
}

/// Data associated with a connection for reassembly.
struct ConnectionData {
    /// The connection state
    connection: Connection,
    /// Tick aligner for incoming packets (server → client)
    incoming_aligner: TickAligner,
    /// Tick aligner for outgoing packets (client → server)
    outgoing_aligner: TickAligner,
    /// Buffer for incoming data
    incoming_buffer: StreamBuffer,
    /// Buffer for outgoing data
    outgoing_buffer: StreamBuffer,
    /// Whether this connection has ever been fully synced (caught from SYN, or
    /// successfully mid-stream synced). Mid-stream ghosts that never sync
    /// should not affect the capture health indicator.
    was_ever_synced: bool,
}

/// Buffer for accumulating and reordering TCP segments.
struct StreamBuffer {
    /// Ordered data ready for processing
    data: Vec<u8>,
    /// Expected next sequence number
    next_seq: u32,
    /// Out-of-order segments waiting to be processed, keyed by sequence number.
    /// Each entry records the capture timestamp of its arriving segment so a
    /// stale gap can be skipped on age (not just on count).
    pending: BTreeMap<u32, (chrono::DateTime<chrono::Utc>, Vec<u8>)>,
    /// Flag to skip until first non-MTU packet
    waiting_for_start: bool,
    /// Maximum buffer size to prevent memory exhaustion (1MB)
    max_buffer_size: usize,
    /// Counter for consecutive invalid packet headers (for detecting stream corruption)
    consecutive_invalid: u32,
}

impl StreamBuffer {
    /// Maximum buffer size (1MB) - prevents runaway memory usage
    const MAX_BUFFER_SIZE: usize = 1024 * 1024;

    /// Maximum consecutive invalid headers before resetting the stream
    const MAX_CONSECUTIVE_INVALID: u32 = 100;

    /// Maximum out-of-order segments to buffer before assuming a segment was
    /// permanently lost and skipping the gap. When the capture itself drops a
    /// segment (e.g. under VPN/high load), the real TCP endpoint has already
    /// ACKed it, so it is never retransmitted on the wire we observe. Without
    /// a skip, `pending` would grow forever and the stream would stall.
    const MAX_PENDING_SEGMENTS: usize = 64;

    /// Maximum age of the oldest pending (out-of-order) segment before the gap
    /// in front of it is assumed permanently lost and skipped. This bounds
    /// recovery latency on low-traffic directions (often outgoing), which may
    /// take a long time to accumulate [`Self::MAX_PENDING_SEGMENTS`] segments.
    const GAP_MAX_AGE_MS: i64 = 1000;

    fn new() -> Self {
        Self {
            data: Vec::with_capacity(64 * 1024),
            next_seq: 0,
            pending: BTreeMap::new(),
            waiting_for_start: true,
            max_buffer_size: Self::MAX_BUFFER_SIZE,
            consecutive_invalid: 0,
        }
    }

    /// Add a segment to the buffer, handling out-of-order delivery.
    fn add_segment(
        &mut self,
        seq: u32,
        payload: &[u8],
        timestamp: chrono::DateTime<chrono::Utc>,
    ) -> bool {
        if payload.is_empty() {
            return false;
        }

        // Safety check: prevent buffer from growing too large
        if self.data.len() + payload.len() > self.max_buffer_size {
            tracing::warn!(
                "Stream buffer overflow ({} + {} > {}), clearing buffer",
                self.data.len(),
                payload.len(),
                self.max_buffer_size
            );
            self.data.clear();
            self.pending.clear();
            self.waiting_for_start = true;
            return false;
        }

        // Skip MTU-sized packets at start
        if self.waiting_for_start {
            if payload.len() < 1460 {
                self.waiting_for_start = false;
                self.next_seq = seq;
            } else {
                return false;
            }
        }

        let expected = self.next_seq;

        if seq == expected {
            // This is the next expected segment
            self.data.extend_from_slice(payload);
            self.next_seq = seq.wrapping_add(payload.len() as u32);
            self.flush_pending();
            true
        } else if seq.wrapping_sub(expected) < 0x8000_0000 {
            // Future segment (ahead of `expected` in wrap-safe sequence space) -
            // buffer it. Using wrapping distance keeps this correct across the
            // 2^32 sequence-number wrap, unlike a numeric `seq > expected`.
            self.pending.insert(seq, (timestamp, payload.to_vec()));
            false
        } else {
            // Old/duplicate segment (behind `expected`)
            false
        }
    }

    /// Flush any pending segments that are now in order.
    ///
    /// Pulls contiguous segments by exact `next_seq` match rather than relying
    /// on `BTreeMap` numeric ordering, which is wrong across the 2^32 sequence
    /// wrap. Strictly-old pending entries (behind `next_seq`) are then pruned so
    /// they cannot linger forever.
    fn flush_pending(&mut self) {
        while let Some((_, payload)) = self.pending.remove(&self.next_seq) {
            self.data.extend_from_slice(&payload);
            self.next_seq = self.next_seq.wrapping_add(payload.len() as u32);
        }

        // Drop any entry that now sits behind next_seq (old/duplicate) using a
        // wrap-safe relative comparison: keep only segments at or ahead of it.
        let next = self.next_seq;
        self.pending
            .retain(|&seq, _| seq.wrapping_sub(next) < 0x8000_0000);
    }

    fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Recover from a permanently-lost segment by skipping the gap.
    ///
    /// A gap is skipped when either:
    /// * out-of-order segments pile up past [`Self::MAX_PENDING_SEGMENTS`]
    ///   (high-traffic directions fill quickly), or
    /// * the oldest pending segment has been waiting longer than
    ///   [`Self::GAP_MAX_AGE_MS`] (so sparse, low-traffic directions still
    ///   recover promptly without first accumulating 64 segments).
    ///
    /// In both cases a segment was almost certainly dropped by the capture and
    /// will never arrive, so `next_seq` is advanced to the earliest buffered
    /// segment and the stream resumes instead of stalling (and leaking memory).
    ///
    /// Skipping discards bytes and breaks RotMG packet framing, so the caller
    /// MUST reset the cipher aligner afterwards to force a fresh tick-based
    /// resync. Returns the number of bytes skipped, or `None` if no skip was
    /// needed. `now` is the timestamp of the segment currently being processed.
    fn try_skip_gap(&mut self, now: chrono::DateTime<chrono::Utc>) -> Option<u32> {
        if self.pending.is_empty() {
            return None;
        }

        let count_exceeded = self.pending.len() > Self::MAX_PENDING_SEGMENTS;
        // Oldest pending entry = largest age relative to `now`.
        let age_exceeded = self
            .pending
            .values()
            .map(|(ts, _)| now.signed_duration_since(*ts).num_milliseconds())
            .max()
            .map(|age| age >= Self::GAP_MAX_AGE_MS)
            .unwrap_or(false);

        if !count_exceeded && !age_exceeded {
            return None;
        }

        let next = match self.pending.first_key_value() {
            Some((&seq, _)) => seq,
            None => return None,
        };

        // Pick the earliest pending segment by TCP sequence distance (wrap-safe)
        // rather than numeric value, in case the sequence space wrapped past 2^32.
        let next = self
            .pending
            .keys()
            .copied()
            .min_by_key(|&seq| seq.wrapping_sub(self.next_seq))
            .unwrap_or(next);

        let skipped = next.wrapping_sub(self.next_seq);
        tracing::debug!(
            "Gap-skip: {} pending segments (count_exceeded={}, age_exceeded={}), advancing seq {} -> {} ({} bytes lost), resyncing",
            self.pending.len(),
            count_exceeded,
            age_exceeded,
            self.next_seq,
            next,
            skipped
        );

        // The bytes already buffered before the hole form an incomplete packet
        // that can never be completed (its tail is in the lost gap). Drop them.
        self.data.clear();
        self.next_seq = next;
        self.consecutive_invalid = 0;
        self.flush_pending();
        Some(skipped)
    }

    /// Record an invalid header and return true if stream should be reset.
    fn record_invalid_header(&mut self) -> bool {
        self.consecutive_invalid += 1;
        if self.consecutive_invalid >= Self::MAX_CONSECUTIVE_INVALID {
            tracing::warn!(
                "Stream appears corrupted ({} consecutive invalid headers), resetting",
                self.consecutive_invalid
            );
            self.data.clear();
            self.pending.clear();
            self.waiting_for_start = true;
            self.consecutive_invalid = 0;
            true
        } else {
            false
        }
    }

    /// Record a valid packet extraction (resets invalid counter).
    fn record_valid_packet(&mut self) {
        self.consecutive_invalid = 0;
    }
}

impl TcpReassembler {
    /// Create a new TCP reassembler.
    pub fn new() -> Self {
        Self {
            connections: HashMap::new(),
            ignored_connections: HashSet::new(),
            recently_closed: HashMap::new(),
            max_connections: 10,
            metrics: CaptureMetrics::new(),
        }
    }

    /// Create a new TCP reassembler with custom max connections.
    pub fn with_max_connections(max_connections: usize) -> Self {
        Self {
            connections: HashMap::new(),
            ignored_connections: HashSet::new(),
            recently_closed: HashMap::new(),
            max_connections,
            metrics: CaptureMetrics::new(),
        }
    }

    /// Process a parsed TCP segment, returning any complete RotMG packets
    /// extracted from the stream.
    pub fn process_segment(&mut self, segment: &TcpSegment) -> Vec<Packet> {
        let (key, is_incoming) = ConnectionKey::from_packet(
            segment.src_ip,
            segment.src_port,
            segment.dst_ip,
            segment.dst_port,
            ROTMG_PORT,
        );

        // Skip connections that have been marked as ignored (secondary game clients)
        if self.ignored_connections.contains(&key) {
            return Vec::new();
        }

        // Handle connection lifecycle based on TCP flags
        if segment.flags.is_syn_only() {
            // Only replace the specific connection, not all connections.
            // Multiple game clients create separate connections that coexist
            // until we identify which is the main account (via HELLO packet).
            if self.connections.remove(&key).is_some() {
                tracing::debug!("Replacing existing connection on new SYN: {}", key);
            }
            self.recently_closed.remove(&key);
            tracing::info!("New connection (SYN): {} - cipher will be synced", key);
            self.create_fresh_connection(key, segment.sequence, 0);
            return Vec::new();
        }

        if segment.flags.is_syn_ack() {
            if let Some(conn_data) = self.connections.get_mut(&key) {
                conn_data.connection.establish(
                    conn_data.outgoing_buffer.next_seq,
                    segment.sequence.wrapping_add(1),
                );
                conn_data.incoming_buffer.next_seq = segment.sequence.wrapping_add(1);
                // Mark incoming cipher as synced since we caught the connection start
                conn_data.incoming_aligner.mark_synced();
                tracing::info!(
                    "Connection established (SYN-ACK): {} - incoming cipher synced",
                    key
                );
            }
            return Vec::new();
        }

        if segment.flags.is_terminating() {
            tracing::debug!("Connection closing: {}", key);
            self.connections.remove(&key);
            self.ignored_connections.remove(&key);
            self.recently_closed.insert(key, std::time::Instant::now());
            self.refresh_sync_state();
            return Vec::new();
        }

        // Get or create connection for data packets
        let conn_data = match self.connections.get_mut(&key) {
            Some(c) => c,
            None => {
                // Don't create ghost connections for stray packets after FIN/RST
                if let Some(closed_at) = self.recently_closed.get(&key) {
                    if closed_at.elapsed().as_secs() < 5 {
                        return Vec::new();
                    }
                    self.recently_closed.remove(&key);
                }
                tracing::debug!("Mid-stream join: {}", key);
                self.create_connection(key, 0, 0);
                match self.connections.get_mut(&key) {
                    Some(c) => c,
                    None => return Vec::new(),
                }
            }
        };

        // Skip empty payloads (pure ACKs)
        if segment.payload.is_empty() {
            return Vec::new();
        }

        // Add data to appropriate buffer
        let (buffer, aligner) = if is_incoming {
            (
                &mut conn_data.incoming_buffer,
                &mut conn_data.incoming_aligner,
            )
        } else {
            (
                &mut conn_data.outgoing_buffer,
                &mut conn_data.outgoing_aligner,
            )
        };

        // Add segment to buffer (handles out-of-order)
        buffer.add_segment(segment.sequence, &segment.payload, segment.timestamp);

        // If a segment was permanently lost (e.g. dropped by the capture under
        // VPN/high load), skip the gap so the stream resumes instead of
        // stalling. Skipping breaks framing, so force the cipher to resync.
        if let Some(bytes_lost) = buffer.try_skip_gap(segment.timestamp) {
            aligner.reset();
            self.metrics.record_gap_skip(bytes_lost);
            self.metrics.record_resync_start();
            tracing::debug!("Cipher aligner reset after gap-skip; awaiting tick resync");
        }

        // Extract complete packets
        let mut packets = Vec::new();
        let mut newly_synced = false;
        loop {
            let was_synced = aligner.is_synced();
            match Self::try_extract_packet_ex(buffer, aligner, is_incoming, segment.timestamp, &key)
            {
                ExtractResult::Packet(packet) => {
                    buffer.record_valid_packet();
                    packets.push(packet);
                }
                ExtractResult::NeedMoreData => break,
                ExtractResult::InvalidHeader => {
                    // Track invalid headers and reset if too many
                    if buffer.record_invalid_header() {
                        // Stream was reset due to corruption, reset aligner too
                        aligner.reset();
                        self.metrics.record_resync_start();
                        tracing::warn!("Cipher reset due to stream corruption");
                    }
                }
                ExtractResult::Unsynced => {
                    if was_synced {
                        // Tick validation failed => cipher just lost sync
                        self.metrics.record_resync_start();
                    }
                    self.metrics.record_unsynced_drop();
                    break;
                }
                ExtractResult::SyncedSkip => {
                    buffer.record_valid_packet();
                    self.metrics.record_resync_success();
                    newly_synced = true;
                }
            }
        }

        if newly_synced {
            if let Some(cd) = self.connections.get_mut(&key) {
                cd.was_ever_synced = true;
            }
        }

        // Aggregate sync state across all active non-ignored connections
        self.refresh_sync_state();
        self.metrics.log_if_needed();

        packets
    }

    /// Recompute the aggregate sync state from all active connections.
    ///
    /// Only considers connections that have been synced at least once (caught
    /// from SYN or successfully mid-stream synced). Mid-stream ghost
    /// connections that never synced are excluded.
    fn refresh_sync_state(&mut self) {
        let relevant: Vec<_> = self
            .connections
            .values()
            .filter(|c| c.was_ever_synced)
            .collect();
        if relevant.is_empty() {
            self.metrics.update_sync_state(true, true);
        } else {
            let (all_in, all_out) = relevant.iter().fold((true, true), |(in_s, out_s), conn| {
                (
                    in_s && conn.incoming_aligner.is_synced(),
                    out_s && conn.outgoing_aligner.is_synced(),
                )
            });
            self.metrics.update_sync_state(all_in, all_out);
        }
    }

    /// Create a new connection entry for a fresh connection (caught from SYN).
    /// The cipher is synced from the start.
    fn create_fresh_connection(&mut self, key: ConnectionKey, client_seq: u32, server_seq: u32) {
        // Remove old connections if at limit
        if self.connections.len() >= self.max_connections {
            if let Some(old_key) = self.connections.keys().next().copied() {
                tracing::debug!("Removing old connection: {}", old_key);
                self.connections.remove(&old_key);
            }
        }

        let mut incoming_buffer = StreamBuffer::new();
        let mut outgoing_buffer = StreamBuffer::new();
        incoming_buffer.next_seq = server_seq;
        outgoing_buffer.next_seq = client_seq;

        // Use synced aligners since we caught the connection from the start
        let conn_data = ConnectionData {
            connection: Connection::new(key),
            incoming_aligner: TickAligner::new_synced(RotmgKeys::INCOMING),
            outgoing_aligner: TickAligner::new_synced(RotmgKeys::OUTGOING),
            incoming_buffer,
            outgoing_buffer,
            was_ever_synced: true,
        };

        self.connections.insert(key, conn_data);
    }

    /// Create a new connection entry for mid-stream join (cipher not synced).
    fn create_connection(&mut self, key: ConnectionKey, client_seq: u32, server_seq: u32) {
        // Remove old connections if at limit
        if self.connections.len() >= self.max_connections {
            if let Some(old_key) = self.connections.keys().next().copied() {
                tracing::debug!("Removing old connection: {}", old_key);
                self.connections.remove(&old_key);
            }
        }

        let mut incoming_buffer = StreamBuffer::new();
        let mut outgoing_buffer = StreamBuffer::new();
        incoming_buffer.next_seq = server_seq;
        outgoing_buffer.next_seq = client_seq;

        // Cipher NOT synced - mid-stream join will use tick alignment
        let conn_data = ConnectionData {
            connection: Connection::new(key),
            incoming_aligner: TickAligner::new(RotmgKeys::INCOMING),
            outgoing_aligner: TickAligner::new(RotmgKeys::OUTGOING),
            incoming_buffer,
            outgoing_buffer,
            was_ever_synced: false,
        };

        self.connections.insert(key, conn_data);
    }

    /// Try to extract a complete RotMG packet from the buffer (extended version).
    /// Returns detailed result for better error handling.
    ///
    /// RotMG packet format:
    /// - 4 bytes: packet size (big-endian u32, includes header)
    /// - 1 byte: packet type ID
    /// - N bytes: payload (where N = size - 5)
    fn try_extract_packet_ex(
        buffer: &mut StreamBuffer,
        aligner: &mut TickAligner,
        is_incoming: bool,
        timestamp: chrono::DateTime<chrono::Utc>,
        key: &ConnectionKey,
    ) -> ExtractResult {
        // Need at least header size (5 bytes)
        if buffer.data.len() < PacketHeader::SIZE {
            return ExtractResult::NeedMoreData;
        }

        // Header is NOT encrypted - read packet size and type
        let packet_size = u32::from_be_bytes([
            buffer.data[0],
            buffer.data[1],
            buffer.data[2],
            buffer.data[3],
        ]) as usize;
        let packet_type = buffer.data[4];

        // Validate packet size
        if packet_size < PacketHeader::SIZE || packet_size > 65535 {
            tracing::debug!(
                "Invalid packet size {}, buffer may be misaligned (buffer len: {})",
                packet_size,
                buffer.data.len()
            );
            // Skip one byte and try to resync
            buffer.data.remove(0);
            return ExtractResult::InvalidHeader;
        }

        // Check if we have the complete packet
        if buffer.data.len() < packet_size {
            return ExtractResult::NeedMoreData;
        }

        // If cipher is not synced, attempt alignment using tick packets
        if !aligner.is_synced() {
            // Check alignment with this packet
            let synced =
                aligner.check_alignment(&buffer.data[..packet_size], packet_size, packet_type);

            if !synced {
                // Not synced yet - discard this packet and continue looking
                buffer.data.drain(..packet_size);
                return ExtractResult::Unsynced;
            }
            // Just synced ON this packet. The aligner has advanced the cipher
            // PAST this packet's payload, so it must NOT be decrypted or emitted
            // (doing so yields garbage and over-advances the cipher, desyncing
            // every following packet). Consume it and resume on the next packet.
            tracing::debug!("Cipher synchronized mid-stream!");
            buffer.data.drain(..packet_size);
            return ExtractResult::SyncedSkip;
        }

        // Cipher is synced. For tick packets, validate the running tick counter
        // using a non-destructive fork BEFORE decrypting. On drift the aligner
        // resets itself; drop this packet without emitting and await a fresh
        // tick-based resync (subsequent buffered packets keep their plaintext
        // headers, so re-sync can still collect the next two ticks).
        if !aligner.validate_synced_tick(&buffer.data[..packet_size], packet_type) {
            buffer.data.drain(..packet_size);
            return ExtractResult::Unsynced;
        }

        // Cipher is synced - we can decrypt properly
        let cipher = match aligner.cipher_mut() {
            Some(c) => c,
            None => return ExtractResult::Unsynced,
        };

        // Extract the complete packet
        let mut packet_data: Vec<u8> = buffer.data.drain(..packet_size).collect();

        // Only decrypt the payload (bytes 5+), NOT the header
        if packet_data.len() > PacketHeader::SIZE {
            cipher.apply(&mut packet_data[PacketHeader::SIZE..]);
        }

        // Parse header from data (header is unencrypted)
        let header = match PacketHeader::parse(&packet_data) {
            Some(h) => h,
            None => return ExtractResult::InvalidHeader,
        };

        // Extract payload (everything after the 5-byte header, now decrypted)
        let payload = packet_data[PacketHeader::SIZE..].to_vec();

        // Build packet with direction info
        let (src_ip, dst_ip, src_port, dst_port) = if is_incoming {
            (
                key.server_ip,
                key.client_ip,
                key.server_port,
                key.client_port,
            )
        } else {
            (
                key.client_ip,
                key.server_ip,
                key.client_port,
                key.server_port,
            )
        };

        ExtractResult::Packet(Packet {
            timestamp,
            incoming: is_incoming,
            header,
            payload,
            src_ip,
            dst_ip,
            src_port,
            dst_port,
        })
    }

    /// Get the number of active connections.
    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }

    /// Get statistics about pending out-of-order segments.
    pub fn pending_segments(&self) -> usize {
        self.connections
            .values()
            .map(|c| c.incoming_buffer.pending_count() + c.outgoing_buffer.pending_count())
            .sum()
    }

    /// Access the capture pipeline diagnostics.
    pub fn metrics(&self) -> &CaptureMetrics {
        &self.metrics
    }

    /// Mutable access to metrics (for the processor to inject pcap/queue stats).
    pub fn metrics_mut(&mut self) -> &mut CaptureMetrics {
        &mut self.metrics
    }

    /// Mark a connection as ignored. All future packets will be silently dropped.
    /// Used for multi-client isolation.
    pub fn ignore_connection(&mut self, key: &ConnectionKey) {
        self.connections.remove(key);
        self.ignored_connections.insert(*key);
    }

    /// Clear the ignored connections set.
    pub fn clear_ignored(&mut self) {
        self.ignored_connections.clear();
    }

    /// Get the number of ignored connections.
    pub fn ignored_count(&self) -> usize {
        self.ignored_connections.len()
    }

    /// Clear all connections and ignored set.
    pub fn clear(&mut self) {
        // Log session summary if any traffic was processed
        let m = &self.metrics;
        if m.gap_skips > 0 || m.resyncs > 0 || m.packets_dropped_unsynced > 0 || m.queue_drops > 0 {
            tracing::info!(
                "[CAPTURE] Session ended | gap-skips: {} ({} bytes) | resyncs: {}/{} ok | \
                 dropped: {} pkts | total unsynced: {}ms | queue drops: {}",
                m.gap_skips,
                m.gap_bytes_lost,
                m.resyncs_ok,
                m.resyncs,
                m.packets_dropped_unsynced,
                m.total_unsync_duration_ms(),
                m.queue_drops,
            );
        }
        self.connections.clear();
        self.ignored_connections.clear();
        self.recently_closed.clear();
        self.metrics.reset();
    }
}

impl Default for TcpReassembler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::super::parser::TcpFlags;
    use super::*;
    use std::net::Ipv4Addr;

    fn make_segment(
        src_ip: [u8; 4],
        src_port: u16,
        dst_ip: [u8; 4],
        dst_port: u16,
        seq: u32,
        flags: TcpFlags,
        payload: &[u8],
    ) -> TcpSegment {
        TcpSegment {
            src_ip: Ipv4Addr::from(src_ip),
            dst_ip: Ipv4Addr::from(dst_ip),
            src_port,
            dst_port,
            sequence: seq,
            acknowledgment: 0,
            flags,
            payload: payload.to_vec(),
            timestamp: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_connection_tracking() {
        let mut reassembler = TcpReassembler::new();

        // Simulate SYN
        let syn_flags = TcpFlags {
            syn: true,
            ack: false,
            ..Default::default()
        };
        let syn = make_segment(
            [192, 168, 1, 100],
            12345,
            [1, 2, 3, 4],
            ROTMG_PORT,
            1000,
            syn_flags,
            &[],
        );
        let packets = reassembler.process_segment(&syn);
        assert!(packets.is_empty());
        assert_eq!(reassembler.connection_count(), 1);

        // Simulate RST (connection close)
        let rst_flags = TcpFlags {
            rst: true,
            ..Default::default()
        };
        let rst = make_segment(
            [192, 168, 1, 100],
            12345,
            [1, 2, 3, 4],
            ROTMG_PORT,
            1001,
            rst_flags,
            &[],
        );
        let packets = reassembler.process_segment(&rst);
        assert!(packets.is_empty());
        assert_eq!(reassembler.connection_count(), 0);
    }

    #[test]
    fn test_stream_buffer_ordering() {
        let mut buffer = StreamBuffer::new();
        buffer.waiting_for_start = false;
        buffer.next_seq = 100;

        // Add in-order segment
        assert!(buffer.add_segment(100, &[1, 2, 3, 4], chrono::Utc::now()));
        assert_eq!(buffer.data, vec![1, 2, 3, 4]);
        assert_eq!(buffer.next_seq, 104);

        // Add out-of-order segment (future)
        assert!(!buffer.add_segment(110, &[11, 12, 13], chrono::Utc::now()));
        assert_eq!(buffer.pending_count(), 1);

        // Add missing segment
        assert!(buffer.add_segment(104, &[5, 6, 7, 8, 9, 10], chrono::Utc::now()));

        // Both should now be in the buffer
        assert_eq!(buffer.data, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]);
        assert_eq!(buffer.pending_count(), 0);
        assert_eq!(buffer.next_seq, 113);
    }

    #[test]
    fn test_stream_buffer_duplicates() {
        let mut buffer = StreamBuffer::new();
        buffer.waiting_for_start = false;
        buffer.next_seq = 100;

        // Add segment
        assert!(buffer.add_segment(100, &[1, 2, 3, 4], chrono::Utc::now()));

        // Add duplicate (should be ignored)
        assert!(!buffer.add_segment(100, &[1, 2, 3, 4], chrono::Utc::now()));
        assert_eq!(buffer.data, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_mtu_skip_at_start() {
        let mut buffer = StreamBuffer::new();

        // Large packet should be skipped
        let large_payload = vec![0u8; 1460];
        assert!(!buffer.add_segment(100, &large_payload, chrono::Utc::now()));
        assert!(buffer.data.is_empty());
        assert!(buffer.waiting_for_start);

        // Small packet should trigger sync
        assert!(buffer.add_segment(200, &[1, 2, 3], chrono::Utc::now()));
        assert!(!buffer.waiting_for_start);
        assert_eq!(buffer.data, vec![1, 2, 3]);
        assert_eq!(buffer.next_seq, 203);
    }

    #[test]
    fn test_gap_skip_no_skip_below_threshold() {
        let mut buffer = StreamBuffer::new();
        buffer.waiting_for_start = false;
        buffer.next_seq = 100;

        // A few out-of-order segments (a real gap) should NOT trigger a skip.
        let now = chrono::Utc::now();
        for i in 0..10u32 {
            let seq = 1000 + i * 10;
            buffer.add_segment(seq, &[i as u8; 10], now);
        }
        assert_eq!(buffer.pending_count(), 10);
        assert!(buffer.try_skip_gap(now).is_none());
        // next_seq is untouched while we wait for the gap to be filled.
        assert_eq!(buffer.next_seq, 100);
    }

    #[test]
    fn test_gap_skip_recovers_after_lost_segment() {
        let mut buffer = StreamBuffer::new();
        buffer.waiting_for_start = false;
        buffer.next_seq = 100;

        // Simulate a permanently-lost segment at seq 100: every later segment
        // piles up in `pending` because the hole is never filled.
        let now = chrono::Utc::now();
        let count = StreamBuffer::MAX_PENDING_SEGMENTS as u32 + 5;
        for i in 0..count {
            let seq = 1000 + i * 10;
            buffer.add_segment(seq, &[1u8; 10], now);
        }
        assert_eq!(buffer.pending_count(), count as usize);

        // The pile-up triggers a gap-skip to the earliest buffered segment.
        assert!(buffer.try_skip_gap(now).is_some());
        assert_eq!(buffer.next_seq, 1000 + count * 10);
        assert_eq!(buffer.pending_count(), 0);
        // Stream resumed: the earliest buffered run was flushed into data.
        assert_eq!(buffer.data.len(), count as usize * 10);
    }

    #[test]
    fn test_gap_skip_age_based_recovery() {
        let mut buffer = StreamBuffer::new();
        buffer.waiting_for_start = false;
        buffer.next_seq = 100;

        // A single out-of-order segment forms a gap (seq 100 is missing).
        let arrived = chrono::Utc::now();
        assert!(!buffer.add_segment(1000, &[1u8; 10], arrived));
        assert_eq!(buffer.pending_count(), 1);

        // Far below the count threshold, but the gap has persisted past the age
        // threshold -> skip so a sparse direction still recovers.
        let now = arrived + chrono::Duration::milliseconds(StreamBuffer::GAP_MAX_AGE_MS + 50);
        assert!(buffer.try_skip_gap(now).is_some());
        assert_eq!(buffer.next_seq, 1010);
        assert_eq!(buffer.pending_count(), 0);
    }

    #[test]
    fn test_gap_skip_fresh_below_count_and_age_no_skip() {
        let mut buffer = StreamBuffer::new();
        buffer.waiting_for_start = false;
        buffer.next_seq = 100;

        let arrived = chrono::Utc::now();
        for i in 0..5u32 {
            buffer.add_segment(1000 + i * 10, &[i as u8; 10], arrived);
        }
        assert_eq!(buffer.pending_count(), 5);

        // Below the count threshold and still within the age window -> no skip.
        let now = arrived + chrono::Duration::milliseconds(StreamBuffer::GAP_MAX_AGE_MS - 100);
        assert!(buffer.try_skip_gap(now).is_none());
        assert_eq!(buffer.next_seq, 100);
    }

    #[test]
    fn test_flush_pending_across_seq_wrap() {
        let mut buffer = StreamBuffer::new();
        buffer.waiting_for_start = false;

        // Start next_seq just below the u32 wrap boundary.
        let base = u32::MAX - 4; // 0xFFFF_FFFB
        buffer.next_seq = base;

        let now = chrono::Utc::now();

        // Future segment B sits at base+5, which wraps around to 0. It must be
        // recognised as ahead of `base` (not behind) and buffered.
        let seq_b = base.wrapping_add(5);
        assert_eq!(seq_b, 0, "test setup: B straddles the wrap boundary");
        assert!(!buffer.add_segment(seq_b, &[9, 9, 9, 9], now));
        assert_eq!(buffer.pending_count(), 1);

        // In-order segment A fills the gap; flush must then pull B across the
        // wrap in the correct order despite its numerically-smaller key.
        assert!(buffer.add_segment(base, &[1, 2, 3, 4, 5], now));
        assert_eq!(buffer.data, vec![1, 2, 3, 4, 5, 9, 9, 9, 9]);
        assert_eq!(buffer.pending_count(), 0);
        assert_eq!(buffer.next_seq, seq_b.wrapping_add(4));
    }
}

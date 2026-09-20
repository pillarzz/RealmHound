//! Real-time capture pipeline diagnostics.
//!
//! Lightweight counters and timing data collected by the reassembler and tick
//! aligner during packet processing. The UI reads a snapshot each frame to
//! surface capture health without requiring log tailing.

use std::time::Instant;

/// How long a gap-skip or kernel drop keeps the health indicator in Degraded.
const DEGRADED_WINDOW_SECS: u64 = 30;

/// Interval for periodic summary logging while not healthy.
const LOG_INTERVAL_SECS: u64 = 10;

/// Accumulated capture-pipeline diagnostics.
///
/// All fields are cheap to copy so the processor can snapshot them each batch.
#[derive(Debug, Clone)]
pub struct CaptureMetrics {
    /// Number of gap-skip events (a permanently lost segment was skipped).
    pub gap_skips: u64,
    /// Total bytes lost to gap-skips.
    pub gap_bytes_lost: u64,
    /// Number of cipher resync attempts (tick aligner reset → search).
    pub resyncs: u64,
    /// Number of successful resyncs (cipher re-aligned after loss).
    pub resyncs_ok: u64,
    /// Packets discarded while the cipher was unsynced.
    pub packets_dropped_unsynced: u64,
    /// Whether the incoming cipher is currently synced.
    pub incoming_synced: bool,
    /// Whether the outgoing cipher is currently synced.
    pub outgoing_synced: bool,
    /// Instant when the current unsynced window started (if not synced).
    unsync_start: Option<Instant>,
    /// Cumulative time spent unsynced (milliseconds).
    pub unsync_duration_ms: u64,
    /// Kernel-level drops reported by pcap.
    pub kernel_drops: u32,
    /// Queue-level drops (bounded queue overflow).
    pub queue_drops: u64,
    /// Last time a gap-skip or drop event occurred (for recent-window health).
    pub last_degraded_event: Option<Instant>,
    /// Previous health state (for transition logging).
    prev_health: CaptureHealth,
    /// Last time a periodic summary was logged.
    last_log: Option<Instant>,
}

impl Default for CaptureMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureMetrics {
    pub fn new() -> Self {
        Self {
            gap_skips: 0,
            gap_bytes_lost: 0,
            resyncs: 0,
            resyncs_ok: 0,
            packets_dropped_unsynced: 0,
            incoming_synced: true,
            outgoing_synced: true,
            unsync_start: None,
            unsync_duration_ms: 0,
            kernel_drops: 0,
            queue_drops: 0,
            last_degraded_event: None,
            prev_health: CaptureHealth::Healthy,
            last_log: None,
        }
    }

    /// Record a gap-skip event with the number of bytes lost.
    pub fn record_gap_skip(&mut self, bytes_lost: u32) {
        self.gap_skips += 1;
        self.gap_bytes_lost += bytes_lost as u64;
        self.last_degraded_event = Some(Instant::now());
    }

    /// Record that a resync attempt has started (cipher was reset).
    pub fn record_resync_start(&mut self) {
        self.resyncs += 1;
    }

    /// Record a successful resync.
    pub fn record_resync_success(&mut self) {
        self.resyncs_ok += 1;
    }

    /// Record a packet dropped because the cipher was not synced.
    pub fn record_unsynced_drop(&mut self) {
        self.packets_dropped_unsynced += 1;
    }

    /// Update the sync state of both directions and accumulate unsynced time.
    pub fn update_sync_state(&mut self, incoming_synced: bool, outgoing_synced: bool) {
        let was_synced = self.incoming_synced && self.outgoing_synced;
        let now_synced = incoming_synced && outgoing_synced;

        self.incoming_synced = incoming_synced;
        self.outgoing_synced = outgoing_synced;

        if was_synced && !now_synced {
            self.unsync_start = Some(Instant::now());
        } else if !was_synced && now_synced {
            if let Some(start) = self.unsync_start.take() {
                self.unsync_duration_ms += start.elapsed().as_millis() as u64;
            }
        }
    }

    /// Snapshot the current unsynced duration (including any ongoing window).
    pub fn total_unsync_duration_ms(&self) -> u64 {
        let ongoing = self
            .unsync_start
            .map(|s| s.elapsed().as_millis() as u64)
            .unwrap_or(0);
        self.unsync_duration_ms + ongoing
    }

    /// Reset all counters (e.g. on capture restart).
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Log a summary on health transitions and periodically while unhealthy.
    ///
    /// Call this after each processing batch. Logs at `warn` level on
    /// transitions and every [`LOG_INTERVAL_SECS`] while degraded/resyncing,
    /// and at `info` level when recovering back to healthy.
    pub fn log_if_needed(&mut self) {
        let current = self.health();

        // Transition logging
        if current != self.prev_health {
            match current {
                CaptureHealth::Healthy => {
                    tracing::info!(
                        "[CAPTURE] Recovered -> Healthy \
                         (session totals: {} gap-skips, {} bytes lost, {} resyncs, \
                         {} packets dropped, {}ms unsynced, {} queue drops)",
                        self.gap_skips,
                        self.gap_bytes_lost,
                        self.resyncs,
                        self.packets_dropped_unsynced,
                        self.total_unsync_duration_ms(),
                        self.queue_drops,
                    );
                }
                CaptureHealth::Resyncing => {
                    tracing::warn!(
                        "[CAPTURE] Cipher LOST SYNC (in={}, out={}) - packets being dropped \
                         (gap-skips: {}, bytes lost: {}, resyncs: {})",
                        self.incoming_synced,
                        self.outgoing_synced,
                        self.gap_skips,
                        self.gap_bytes_lost,
                        self.resyncs,
                    );
                }
                CaptureHealth::Degraded => {
                    tracing::warn!(
                        "[CAPTURE] Degraded - recent packet loss \
                         (gap-skips: {}, bytes lost: {}, queue drops: {})",
                        self.gap_skips,
                        self.gap_bytes_lost,
                        self.queue_drops,
                    );
                }
            }
            self.prev_health = current;
            self.last_log = Some(Instant::now());
            return;
        }

        // Periodic summary while unhealthy
        if current == CaptureHealth::Healthy {
            return;
        }
        let should_log = self
            .last_log
            .map(|t| t.elapsed().as_secs() >= LOG_INTERVAL_SECS)
            .unwrap_or(true);
        if should_log {
            tracing::warn!(
                "[CAPTURE] Status: {:?} | gap-skips: {} ({} bytes) | resyncs: {}/{} ok | \
                 dropped: {} pkts | unsynced: {}ms | queue drops: {}",
                current,
                self.gap_skips,
                self.gap_bytes_lost,
                self.resyncs_ok,
                self.resyncs,
                self.packets_dropped_unsynced,
                self.total_unsync_duration_ms(),
                self.queue_drops,
            );
            self.last_log = Some(Instant::now());
        }
    }
}

/// Compact health summary for the UI status bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureHealth {
    /// Everything operating normally.
    Healthy,
    /// Cipher currently re-syncing (packets being dropped).
    Resyncing,
    /// Recent packet loss detected but currently synced.
    Degraded,
}

impl CaptureMetrics {
    /// Derive the overall capture health from current metrics.
    ///
    /// Only the incoming direction (server→client) matters for functionality
    /// (loot, map, dungeon tracking). Outgoing desync is minor.
    pub fn health(&self) -> CaptureHealth {
        if !self.incoming_synced {
            CaptureHealth::Resyncing
        } else if let Some(last) = self.last_degraded_event {
            if last.elapsed().as_secs() < DEGRADED_WINDOW_SECS {
                CaptureHealth::Degraded
            } else {
                CaptureHealth::Healthy
            }
        } else {
            CaptureHealth::Healthy
        }
    }
}

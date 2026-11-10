//! Always-on pipeline health, compiled into every build.
//!
//! `latency-diagnostics` may add verbose timing, but the baseline numbers here
//! must exist without it (see `docs/design/ui-pipeline.md` §5.10): frame gap,
//! drain/capped frames, queue depth and drops, capture state and a
//! persistence-failure slot.
//!
//! Counters are atomics updated on the hot path; readers take a cheap copy.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the UI health window is rolled into a log line.
pub const LOG_INTERVAL: Duration = Duration::from_secs(30);

/// A frame gap at or above this value is called out as a stall when the window
/// also shows a backlog. Idle demand-driven frames are expected to be far apart.
const STALL_WARN_GAP: Duration = Duration::from_secs(2);

/// Lifecycle of the capture source as observed by the worker.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureState {
    /// No capture source is attached.
    Stopped = 0,
    /// The worker is draining a capture source.
    Active = 1,
    /// The source ended unexpectedly and has not been replaced.
    Closed = 2,
}

impl CaptureState {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Active,
            2 => Self::Closed,
            _ => Self::Stopped,
        }
    }

    /// Stable human-readable label for logs and the status bar.
    pub fn label(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Active => "active",
            Self::Closed => "closed",
        }
    }
}

/// A point-in-time copy of the always-on health counters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthSnapshot {
    /// Gap between the last two UI frames.
    pub last_frame_gap: Duration,
    /// Largest frame gap observed since startup.
    pub peak_frame_gap: Duration,
    /// Frames that drained at least one worker update.
    pub drain_frames: u64,
    /// Worker updates applied across all drain frames.
    pub drained_updates: u64,
    /// Drain frames that hit the per-frame cap (more work may remain).
    pub capped_drain_frames: u64,
    /// Updates currently waiting for the UI.
    pub queue_depth: usize,
    /// Cumulative updates dropped by the bounded UI queue.
    pub queue_drops: u64,
    /// Cumulative Live Feed window evictions.
    pub live_window_evictions: u64,
    /// Cumulative stale audio command drops.
    pub audio_stale_drops: u64,
    /// Cumulative audio channel-full drops.
    pub audio_full_drops: u64,
    /// Peak worker snapshot-publish duration within the window.
    pub peak_snapshot_publish: Duration,
    /// Peak worker snapshot-lock wait within the window.
    pub peak_snapshot_lock_wait: Duration,
    /// Current capture source state.
    pub capture: CaptureState,
    /// Cumulative persistence failures observed by the app/worker.
    pub persistence_failures: u64,
    /// Most recent persistence failure, if any.
    pub last_persistence_error: Option<String>,
}

/// Health counters shared by the UI thread and the worker.
pub struct HealthCounters {
    last_frame_gap_ms: AtomicU64,
    peak_frame_gap_ms: AtomicU64,
    drain_frames: AtomicU64,
    drained_updates: AtomicU64,
    capped_drain_frames: AtomicU64,
    live_window_evictions: AtomicU64,
    audio_stale_drops: AtomicU64,
    audio_full_drops: AtomicU64,
    snapshot_publish_us: AtomicU64,
    snapshot_lock_wait_us: AtomicU64,
    capture: AtomicU8,
    persistence_failures: AtomicU64,
    last_persistence_error: Mutex<Option<String>>,
}

impl Default for HealthCounters {
    fn default() -> Self {
        Self {
            last_frame_gap_ms: AtomicU64::new(0),
            peak_frame_gap_ms: AtomicU64::new(0),
            drain_frames: AtomicU64::new(0),
            drained_updates: AtomicU64::new(0),
            capped_drain_frames: AtomicU64::new(0),
            live_window_evictions: AtomicU64::new(0),
            audio_stale_drops: AtomicU64::new(0),
            audio_full_drops: AtomicU64::new(0),
            snapshot_publish_us: AtomicU64::new(0),
            snapshot_lock_wait_us: AtomicU64::new(0),
            capture: AtomicU8::new(CaptureState::Stopped as u8),
            persistence_failures: AtomicU64::new(0),
            last_persistence_error: Mutex::new(None),
        }
    }
}

impl HealthCounters {
    /// Fresh counters: no traffic, no persistence failures, capture stopped.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one UI frame and the drain work it performed.
    pub fn record_frame(&self, gap: Duration, drained: usize, capped: bool) {
        let gap_ms = u64::try_from(gap.as_millis()).unwrap_or(u64::MAX);
        self.last_frame_gap_ms.store(gap_ms, Ordering::Relaxed);
        self.peak_frame_gap_ms.fetch_max(gap_ms, Ordering::Relaxed);
        if drained > 0 {
            self.drain_frames.fetch_add(1, Ordering::Relaxed);
            self.drained_updates
                .fetch_add(drained as u64, Ordering::Relaxed);
        }
        if capped {
            self.capped_drain_frames.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record a persistence failure and keep the most recent message.
    pub fn record_persistence_failure(&self, error: impl Into<String>) {
        self.persistence_failures.fetch_add(1, Ordering::Relaxed);
        let mut last = self
            .last_persistence_error
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *last = Some(error.into());
    }

    /// Set the cumulative Live Feed window eviction count.
    pub fn set_live_window_evictions(&self, count: u64) {
        self.live_window_evictions.store(count, Ordering::Relaxed);
    }

    /// Set the cumulative audio `(stale, full)` drop counts.
    pub fn set_audio_drops(&self, stale: u64, full: u64) {
        self.audio_stale_drops.store(stale, Ordering::Relaxed);
        self.audio_full_drops.store(full, Ordering::Relaxed);
    }

    /// Raise the window peak worker snapshot-publish duration.
    pub fn record_snapshot_publish(&self, took: Duration) {
        self.snapshot_publish_us
            .fetch_max(took.as_micros() as u64, Ordering::Relaxed);
    }

    /// Raise the window peak worker snapshot-lock wait.
    pub fn record_snapshot_lock_wait(&self, wait: Duration) {
        self.snapshot_lock_wait_us
            .fetch_max(wait.as_micros() as u64, Ordering::Relaxed);
    }

    fn reset_snapshot_peaks(&self) {
        self.snapshot_publish_us.store(0, Ordering::Relaxed);
        self.snapshot_lock_wait_us.store(0, Ordering::Relaxed);
    }

    /// Update the observed capture source state.
    pub fn set_capture_state(&self, state: CaptureState) {
        self.capture.store(state as u8, Ordering::Relaxed);
    }

    /// Copy the counters, filling in queue values owned by the UI receiver.
    pub fn snapshot(&self, queue_depth: usize, queue_drops: u64) -> HealthSnapshot {
        HealthSnapshot {
            last_frame_gap: Duration::from_millis(self.last_frame_gap_ms.load(Ordering::Relaxed)),
            peak_frame_gap: Duration::from_millis(self.peak_frame_gap_ms.load(Ordering::Relaxed)),
            drain_frames: self.drain_frames.load(Ordering::Relaxed),
            drained_updates: self.drained_updates.load(Ordering::Relaxed),
            capped_drain_frames: self.capped_drain_frames.load(Ordering::Relaxed),
            queue_depth,
            queue_drops,
            live_window_evictions: self.live_window_evictions.load(Ordering::Relaxed),
            audio_stale_drops: self.audio_stale_drops.load(Ordering::Relaxed),
            audio_full_drops: self.audio_full_drops.load(Ordering::Relaxed),
            peak_snapshot_publish: Duration::from_micros(
                self.snapshot_publish_us.load(Ordering::Relaxed),
            ),
            peak_snapshot_lock_wait: Duration::from_micros(
                self.snapshot_lock_wait_us.load(Ordering::Relaxed),
            ),
            capture: CaptureState::from_u8(self.capture.load(Ordering::Relaxed)),
            persistence_failures: self.persistence_failures.load(Ordering::Relaxed),
            last_persistence_error: self
                .last_persistence_error
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        }
    }
}

/// Summary of one rolled health window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthSummary {
    /// Window length.
    pub window: Duration,
    /// Frames observed during the window.
    pub frames: u64,
    /// Updates drained during the window.
    pub drained_updates: u64,
    /// Drain frames that hit the per-frame cap.
    pub capped_drain_frames: u64,
    /// Largest frame gap in the window.
    pub peak_frame_gap: Duration,
    /// Largest post-drain queue depth in the window.
    pub peak_queue_depth: usize,
    /// Queue drops that happened during the window.
    pub queue_drops_delta: u64,
    /// Cumulative Live Feed window evictions at window end.
    pub live_window_evictions: u64,
    /// Cumulative audio stale drops at window end.
    pub audio_stale_drops: u64,
    /// Cumulative audio channel-full drops at window end.
    pub audio_full_drops: u64,
    /// Peak worker snapshot-publish duration in the window.
    pub peak_snapshot_publish: Duration,
    /// Peak worker snapshot-lock wait in the window.
    pub peak_snapshot_lock_wait: Duration,
    /// Capture state at window end.
    pub capture: CaptureState,
    /// Cumulative persistence failures at window end.
    pub persistence_failures: u64,
}

/// Whether the periodic `[HEALTH][*]` and pipeline trace lines are emitted.
///
/// Counters and the status bar always run; the log output is opt-in for
/// investigations via `REALMHOUND_PIPELINE_LOGS=1` (anything but 0/false/off).
pub fn pipeline_logs_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("REALMHOUND_PIPELINE_LOGS")
            .map(|value| parse_flag(&value))
            .unwrap_or(false)
    })
}

fn parse_flag(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "off" | "no"
    )
}

impl HealthSummary {
    /// Emit the window as a throttled log line, when pipeline logging is on.
    pub fn emit(&self, active_tab: &str, focused: bool, minimized: Option<bool>) {
        if !pipeline_logs_enabled() {
            return;
        }
        let minimized = minimized
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        // Warn only on actual backlog pressure; a large gap with an empty queue
        // is just a naturally idle demand-driven window.
        let backlog = self.capped_drain_frames > 0 || self.queue_drops_delta > 0;
        if backlog && self.peak_frame_gap >= STALL_WARN_GAP {
            tracing::warn!(
                "[HEALTH][UI] window_ms={} frames={} drained_updates={} capped_frames={} \
                 peak_frame_gap_ms={} peak_publish_ms={} peak_lock_wait_ms={} \
                 peak_queue_depth={} queue_drops={} live_window_evictions={} \
                 audio_stale_drops={} audio_full_drops={} capture={} persistence_failures={} \
                 active_tab={} viewport_focused={} viewport_minimized={}",
                self.window.as_millis(),
                self.frames,
                self.drained_updates,
                self.capped_drain_frames,
                self.peak_frame_gap.as_millis(),
                self.peak_snapshot_publish.as_millis(),
                self.peak_snapshot_lock_wait.as_millis(),
                self.peak_queue_depth,
                self.queue_drops_delta,
                self.live_window_evictions,
                self.audio_stale_drops,
                self.audio_full_drops,
                self.capture.label(),
                self.persistence_failures,
                active_tab,
                focused,
                minimized
            );
        } else {
            tracing::info!(
                "[HEALTH][UI] window_ms={} frames={} drained_updates={} capped_frames={} \
                 peak_frame_gap_ms={} peak_publish_ms={} peak_lock_wait_ms={} \
                 peak_queue_depth={} queue_drops={} live_window_evictions={} \
                 audio_stale_drops={} audio_full_drops={} capture={} persistence_failures={} \
                 active_tab={} viewport_focused={} viewport_minimized={}",
                self.window.as_millis(),
                self.frames,
                self.drained_updates,
                self.capped_drain_frames,
                self.peak_frame_gap.as_millis(),
                self.peak_snapshot_publish.as_millis(),
                self.peak_snapshot_lock_wait.as_millis(),
                self.peak_queue_depth,
                self.queue_drops_delta,
                self.live_window_evictions,
                self.audio_stale_drops,
                self.audio_full_drops,
                self.capture.label(),
                self.persistence_failures,
                active_tab,
                focused,
                minimized
            );
        }
    }
}

/// UI-side always-on health recorder.
pub struct HealthMonitor {
    counters: Arc<HealthCounters>,
    last_frame: Option<Instant>,
    window_started: Option<Instant>,
    window_frames: u64,
    window_updates: u64,
    window_capped: u64,
    window_peak_gap: Duration,
    window_peak_queue: usize,
    window_drops_start: u64,
    last_gap: Duration,
}

impl HealthMonitor {
    /// Build a monitor over shared worker/UI counters.
    pub fn new(counters: Arc<HealthCounters>) -> Self {
        Self {
            counters,
            last_frame: None,
            window_started: None,
            window_frames: 0,
            window_updates: 0,
            window_capped: 0,
            window_peak_gap: Duration::ZERO,
            window_peak_queue: 0,
            window_drops_start: 0,
            last_gap: Duration::ZERO,
        }
    }

    /// Shared counters, so the worker can record capture/persistence state.
    pub fn counters(&self) -> &Arc<HealthCounters> {
        &self.counters
    }

    /// Gap measured on the most recent recorded frame.
    pub fn last_frame_gap(&self) -> Duration {
        self.last_gap
    }

    /// Record one frame. Returns a summary once [`LOG_INTERVAL`] elapses and
    /// the window saw drain activity or new queue drops.
    pub fn record_frame(
        &mut self,
        now: Instant,
        drained: usize,
        capped: bool,
        queue_depth: usize,
        queue_drops: u64,
    ) -> Option<HealthSummary> {
        let gap = self
            .last_frame
            .map(|last| now.saturating_duration_since(last))
            .unwrap_or_default();
        self.last_frame = Some(now);
        self.last_gap = gap;
        self.counters.record_frame(gap, drained, capped);

        let started = *self.window_started.get_or_insert(now);
        self.window_frames = self.window_frames.saturating_add(1);
        self.window_updates = self.window_updates.saturating_add(drained as u64);
        self.window_capped = self.window_capped.saturating_add(u64::from(capped));
        self.window_peak_gap = self.window_peak_gap.max(gap);
        self.window_peak_queue = self.window_peak_queue.max(queue_depth);

        let window = now.saturating_duration_since(started);
        if window < LOG_INTERVAL {
            return None;
        }
        let drops_delta = queue_drops.saturating_sub(self.window_drops_start);
        let has_activity = self.window_updates > 0 || drops_delta > 0;
        let counters = self.counters.snapshot(0, 0);
        let summary = has_activity.then(|| HealthSummary {
            window,
            frames: self.window_frames,
            drained_updates: self.window_updates,
            capped_drain_frames: self.window_capped,
            peak_frame_gap: self.window_peak_gap,
            peak_queue_depth: self.window_peak_queue,
            queue_drops_delta: drops_delta,
            live_window_evictions: counters.live_window_evictions,
            audio_stale_drops: counters.audio_stale_drops,
            audio_full_drops: counters.audio_full_drops,
            peak_snapshot_publish: counters.peak_snapshot_publish,
            peak_snapshot_lock_wait: counters.peak_snapshot_lock_wait,
            capture: counters.capture,
            persistence_failures: counters.persistence_failures,
        });
        self.window_started = Some(now);
        self.window_frames = 0;
        self.window_updates = 0;
        self.window_capped = 0;
        self.window_peak_gap = Duration::ZERO;
        self.window_peak_queue = 0;
        self.window_drops_start = queue_drops;
        self.counters.reset_snapshot_peaks();
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing::contract::{UiPayload, UiUpdate};
    use crate::processing::ui_channel;

    fn update(seq: u64) -> UiUpdate {
        UiUpdate {
            seq,
            #[cfg(feature = "latency-diagnostics")]
            enqueued_at: None,
            payload: UiPayload::LootBatchAdded,
        }
    }

    #[test]
    fn drop_injection_reports_queue_depth_and_drops() {
        let (tx, rx) = ui_channel::channel(3);
        // Inject two overflow drops: sending five into a cap-three queue keeps
        // the newest three and reports the two evicted oldest updates.
        for seq in 0..5 {
            tx.send(update(seq)).ok();
        }
        assert_eq!(rx.dropped(), 2);
        assert_eq!(rx.len(), 3);

        let counters = Arc::new(HealthCounters::new());
        let mut monitor = HealthMonitor::new(counters.clone());
        let now = Instant::now();
        assert!(monitor
            .record_frame(now, 3, false, rx.len(), rx.dropped())
            .is_none());

        let snapshot = counters.snapshot(rx.len(), rx.dropped());
        assert_eq!(snapshot.queue_depth, 3);
        assert_eq!(snapshot.queue_drops, 2);
        assert_eq!(snapshot.drain_frames, 1);
        assert_eq!(snapshot.drained_updates, 3);

        let summary = monitor
            .record_frame(now + LOG_INTERVAL, 2, false, 0, rx.dropped())
            .unwrap();
        assert_eq!(summary.frames, 2);
        assert_eq!(summary.drained_updates, 5);
        assert_eq!(summary.peak_queue_depth, 3);
        assert_eq!(summary.queue_drops_delta, 2);
    }

    #[test]
    fn persistence_failure_slot_keeps_last_error() {
        let counters = HealthCounters::new();
        assert_eq!(counters.snapshot(0, 0).persistence_failures, 0);
        counters.record_persistence_failure("loot insert failed");
        counters.record_persistence_failure("account save failed");
        let snapshot = counters.snapshot(0, 0);
        assert_eq!(snapshot.persistence_failures, 2);
        assert_eq!(
            snapshot.last_persistence_error.as_deref(),
            Some("account save failed")
        );
    }

    #[test]
    fn snapshot_peaks_keep_the_window_maximum() {
        let counters = HealthCounters::new();
        counters.record_snapshot_publish(Duration::from_millis(4));
        counters.record_snapshot_publish(Duration::from_millis(2));
        counters.record_snapshot_lock_wait(Duration::from_micros(750));
        let snapshot = counters.snapshot(0, 0);
        assert_eq!(snapshot.peak_snapshot_publish, Duration::from_millis(4));
        assert_eq!(snapshot.peak_snapshot_lock_wait, Duration::from_micros(750));
    }

    #[test]
    fn pipeline_log_flag_is_opt_in() {
        for off in ["", "0", "false", "off", "no", " FALSE "] {
            assert!(!parse_flag(off), "{off:?} must keep logging off");
        }
        for on in ["1", "true", "yes", "on", "anything"] {
            assert!(parse_flag(on), "{on:?} enables pipeline logging");
        }
    }

    #[test]
    fn capture_state_round_trips() {
        let counters = HealthCounters::new();
        assert_eq!(counters.snapshot(0, 0).capture, CaptureState::Stopped);
        counters.set_capture_state(CaptureState::Active);
        assert_eq!(counters.snapshot(0, 0).capture, CaptureState::Active);
        counters.set_capture_state(CaptureState::Closed);
        assert_eq!(counters.snapshot(0, 0).capture, CaptureState::Closed);
    }

    #[test]
    fn idle_window_does_not_emit_a_summary() {
        let counters = Arc::new(HealthCounters::new());
        let mut monitor = HealthMonitor::new(counters);
        let now = Instant::now();
        assert!(monitor.record_frame(now, 0, false, 0, 0).is_none());
        assert!(monitor
            .record_frame(now + LOG_INTERVAL, 0, false, 0, 0)
            .is_none());
    }
}

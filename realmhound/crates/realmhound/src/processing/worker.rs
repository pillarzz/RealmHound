//! The packet-processing worker thread.
//!
//! Owns the [`PacketProcessor`] (and therefore the whole packet model: session,
//! reassembler, router, loot tracker, account/vault data) and runs the pipeline
//! on a dedicated thread so processing, loot recording and sound firing happen
//! in real time regardless of the UI window state.
//!
//! Communication:
//! - capture -> worker: a [`PacketReceiver`] handed in via [`ControlMsg::StartCapture`].
//! - worker -> UI: an ordered [`UiUpdate`] channel + a shared latest-value
//!   [`ModelSnapshot`].
//! - UI -> worker: a [`ControlMsg`] channel (edits, lifecycle, shutdown).
//! - worker -> audio: an [`AudioSender`] (audio commands are routed here, never
//!   forwarded to the UI).

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use eframe::egui;
use realmhound_core::account::AccountContext;
use realmhound_core::capture::PacketReceiver;
use realmhound_core::GameEvent;

use crate::audio::AudioSender;
use crate::panels::ActiveTab;

use super::contract::{
    ControlMsg, ModelSnapshot, SelectedIdentity, ShutdownReport, UiPayload, UiUpdate,
};
use super::processor::PacketProcessor;
use super::ui_channel::{self, UiReceiver, UiSender};

/// How long the worker blocks waiting for packets before looping to service
/// control messages and periodic tasks.
const RECV_TIMEOUT: Duration = Duration::from_millis(100);
/// Max packets drained per pipeline batch (keeps batches bounded).
const MAX_PACKETS_PER_BATCH: usize = 256;
/// Periodic account-data save interval (interim S2 persistence).
const SAVE_INTERVAL: Duration = Duration::from_secs(60);
/// Bounded-staleness fallback: when a batch produced only updates irrelevant to
/// the active tab, drain the queue (and refresh background tabs) at ~1 Hz rather
/// than waking every frame. True idle (no traffic) arms nothing, so static tabs
/// still sleep fully while connected.
const SUPPRESSED_DRAIN_INTERVAL: Duration = Duration::from_millis(1000);
#[cfg(feature = "latency-diagnostics")]
const PIPELINE_WARN_AGE: Duration = Duration::from_millis(500);
#[cfg(feature = "latency-diagnostics")]
const PIPELINE_WARN_WORK: Duration = Duration::from_millis(250);
#[cfg(feature = "latency-diagnostics")]
const PIPELINE_EXTREME_AGE: Duration = Duration::from_secs(2);
#[cfg(feature = "latency-diagnostics")]
const PIPELINE_EXTREME_WORK: Duration = Duration::from_secs(1);
#[cfg(feature = "latency-diagnostics")]
const PIPELINE_WARN_THROTTLE: Duration = Duration::from_secs(2);
#[cfg(feature = "latency-diagnostics")]
const HEALTH_MIN_INTERVAL: Duration = Duration::from_secs(30);

/// Signal delivered to a freshly spawned, still-inactive worker deciding
/// whether it begins processing or exits without ever persisting.
enum StartSignal {
    /// Begin the normal processing loop.
    Activate,
    /// Exit immediately without any flush or persistence (staged cancel).
    Cancel,
}

/// Handle the UI keeps to talk to the worker thread.
pub struct WorkerHandle {
    /// UI -> worker control channel (edits + lifecycle).
    control_tx: Sender<ControlMsg>,
    /// One-shot activation/cancel signal for the staged worker.
    start_tx: Sender<StartSignal>,
    /// Worker -> UI ordered update stream (bounded, drop-oldest on overflow).
    pub ui_rx: UiReceiver,
    /// Shared latest-value model snapshot.
    pub snapshot: Arc<Mutex<ModelSnapshot>>,
    /// Signalled by the worker once its final flush is done, carrying the outcome.
    shutdown_done: Receiver<ShutdownReport>,
    /// Join handle for the worker thread.
    join: Option<JoinHandle<()>>,
}

impl WorkerHandle {
    /// Send a control message to the worker (best-effort).
    pub fn send_control(&self, msg: ControlMsg) {
        if self.control_tx.send(msg).is_err() {
            tracing::warn!("[WORKER] control channel closed; message dropped");
        }
    }

    /// A cloned control sender for use from background threads (e.g. an HTTP
    /// fetch worker that posts its parsed result straight to the processor).
    pub fn control_sender(&self) -> Sender<ControlMsg> {
        self.control_tx.clone()
    }

    /// Activate a staged worker so it begins processing and persisting. Called
    /// only after last-opened and retention metadata have been recorded.
    pub fn activate(&self) {
        if self.start_tx.send(StartSignal::Activate).is_err() {
            tracing::warn!("[WORKER] staged worker gone; activation dropped");
        }
    }

    /// Cancel a staged (never-activated) worker and join it synchronously, so no
    /// detached thread keeps holding the profile lock. The worker exits without
    /// any flush or persistence because it was never activated.
    pub fn cancel_staged(mut self) {
        let _ = self.start_tx.send(StartSignal::Cancel);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }

    /// Ask the worker to flush and exit, then join with a timeout. On timeout the
    /// thread is detached and [`ShutdownReport::TimedOut`] returned.
    pub fn shutdown(&mut self, timeout: Duration) -> ShutdownReport {
        let _ = self.control_tx.send(ControlMsg::Shutdown);
        match self.shutdown_done.recv_timeout(timeout) {
            Ok(report) => {
                if let Some(join) = self.join.take() {
                    let _ = join.join();
                }
                report
            }
            Err(_) => {
                tracing::warn!("[WORKER] shutdown timed out after {:?}; detaching", timeout);
                ShutdownReport::TimedOut
            }
        }
    }
}

/// Spawn the worker thread, taking ownership of the model `processor`.
///
/// `ctx` is an egui context clone used to `request_repaint()` when visible
/// updates are produced (so the UI wakes up even while minimized). `audio_tx`
/// routes worker-emitted sounds to the dedicated audio thread. `account` is the
/// shared immutable account context; the worker holds a clone so the exclusive
/// profile lock inside it stays held for the worker's entire lifetime (including
/// after a detached shutdown).
///
/// The worker is created staged/inactive: it processes and persists nothing
/// until [`WorkerHandle::activate`] is called. Thread creation is fallible so a
/// spawn failure is surfaced before any startup metadata is recorded. If startup
/// metadata later fails, [`WorkerHandle::cancel_staged`] exits and joins the
/// worker without it ever persisting.
pub fn spawn(
    mut processor: PacketProcessor,
    ctx: egui::Context,
    audio_tx: AudioSender,
    active_tab: Arc<AtomicU8>,
    account: Arc<AccountContext>,
) -> std::io::Result<WorkerHandle> {
    processor.set_selected_scope(account.account_key(), account.account_id().clone());

    let (control_tx, control_rx) = channel::<ControlMsg>();
    let (start_tx, start_rx) = channel::<StartSignal>();
    let (ui_tx, ui_rx) = ui_channel::channel(ui_channel::DEFAULT_BOUND);
    let (done_tx, done_rx) = channel::<ShutdownReport>();

    let selected_identity =
        SelectedIdentity::new(account.account_key(), account.account_id().clone());
    let snapshot = Arc::new(Mutex::new(ModelSnapshot::new(
        selected_identity,
        processor.view_state(),
        processor.account_data().clone(),
        processor.live_vault_data(),
        processor.live_char_id(),
    )));

    let worker = Worker {
        processor,
        ui_tx,
        audio_tx,
        snapshot: snapshot.clone(),
        ctx,
        active_tab,
        account,
        receiver: None,
        #[cfg(feature = "latency-diagnostics")]
        capture_drops: CaptureDropTotals::default(),
        last_save: Instant::now(),
        pub_char_gen: u64::MAX,
        pub_account_gen: u64::MAX,
        pub_vault_gen: u64::MAX,
        pub_view: None,
        pub_mission_gen: u64::MAX,
        #[cfg(feature = "latency-diagnostics")]
        latency: LatencyDiagnostics::default(),
    };

    let join = std::thread::Builder::new()
        .name("realmhound-worker".into())
        .spawn(move || worker.run(control_rx, done_tx, start_rx))?;

    Ok(WorkerHandle {
        control_tx,
        start_tx,
        ui_rx,
        snapshot,
        shutdown_done: done_rx,
        join: Some(join),
    })
}

struct Worker {
    processor: PacketProcessor,
    ui_tx: UiSender,
    audio_tx: AudioSender,
    snapshot: Arc<Mutex<ModelSnapshot>>,
    ctx: egui::Context,
    /// Shared active-tab signal written by the UI thread; read to decide whether
    /// an update is worth waking the UI for (relevance-gated repaints).
    active_tab: Arc<AtomicU8>,
    /// Shared immutable account context. Held so the exclusive profile lock
    /// inside it lives as long as the worker (including after a detached
    /// shutdown), keeping the selected profile owned by this process.
    #[allow(dead_code)]
    account: Arc<AccountContext>,
    receiver: Option<PacketReceiver>,
    #[cfg(feature = "latency-diagnostics")]
    capture_drops: CaptureDropTotals,
    last_save: Instant,
    pub_char_gen: u64,
    pub_account_gen: u64,
    pub_vault_gen: u64,
    pub_view: Option<super::contract::ViewState>,
    pub_mission_gen: u64,
    #[cfg(feature = "latency-diagnostics")]
    latency: LatencyDiagnostics,
}

#[cfg(feature = "latency-diagnostics")]
#[derive(Default)]
struct LatencyDiagnostics {
    last_warn: Option<Instant>,
    backlog: Option<Backlog>,
    large_remaining_since: Option<Instant>,
    clock_skew_logged: bool,
    extreme_active: bool,
    last_extreme_transition_warn: Option<Instant>,
    previous_capture_drops: u64,
    capture_drop_episode_active: bool,
    health: PipelineHealth,
}

#[cfg(feature = "latency-diagnostics")]
struct Backlog {
    started: Instant,
    peak_pre_queue: Duration,
    peak_queue: Duration,
    peak_age: Duration,
    clock_skew_seen: bool,
    skew_skipped_packets: usize,
    peak_remaining: usize,
    peak_drops: u64,
    capture_drop_delta: u64,
    slowest_process: Duration,
    slowest_dispatch: Duration,
    slowest_snapshot: Duration,
    slowest_save: Duration,
    slowest_misc: Duration,
    slowest_loop: Duration,
}

#[cfg(feature = "latency-diagnostics")]
#[derive(Default)]
struct PacketLatency {
    pre_queue: Duration,
    queue: Duration,
    age: Duration,
    clock_skew: bool,
    skew_skipped_packets: usize,
    long_age: bool,
}

#[cfg(feature = "latency-diagnostics")]
#[derive(Default)]
struct LoopSegments {
    process: Duration,
    dispatch: Duration,
    snapshot: Duration,
    save: Duration,
    misc: Duration,
    total: Duration,
}

#[cfg(feature = "latency-diagnostics")]
#[derive(Default)]
struct CaptureDropTotals {
    completed_sources: u64,
    current_source: u64,
}

#[cfg(feature = "latency-diagnostics")]
impl CaptureDropTotals {
    fn observe(&mut self, source_drops: u64) -> u64 {
        if source_drops < self.current_source {
            self.completed_sources = self.completed_sources.saturating_add(self.current_source);
        }
        self.current_source = source_drops;
        self.total()
    }

    fn finish_source(&mut self, source_drops: u64) -> u64 {
        self.completed_sources = self
            .completed_sources
            .saturating_add(source_drops.max(self.current_source));
        self.current_source = 0;
        self.total()
    }

    fn total(&self) -> u64 {
        self.completed_sources.saturating_add(self.current_source)
    }
}

#[cfg(feature = "latency-diagnostics")]
#[derive(Default)]
struct PipelineHealth {
    started: Option<Instant>,
    packets: u64,
    batches: u64,
    peak_pre_queue: Duration,
    peak_queue: Duration,
    peak_age: Duration,
    peak_remaining: usize,
    capture_drop_delta: u64,
    capture_drops: u64,
    skew_skipped_packets: usize,
    peak_process: Duration,
    peak_dispatch: Duration,
    peak_snapshot: Duration,
    peak_save: Duration,
    peak_misc: Duration,
    peak_total: Duration,
}

#[cfg(feature = "latency-diagnostics")]
struct PipelineHealthSummary {
    window: Duration,
    packets: u64,
    batches: u64,
    peak_pre_queue: Duration,
    peak_queue: Duration,
    peak_age: Duration,
    peak_remaining: usize,
    capture_drop_delta: u64,
    capture_drops: u64,
    skew_skipped_packets: usize,
    peak_process: Duration,
    peak_dispatch: Duration,
    peak_snapshot: Duration,
    peak_save: Duration,
    peak_misc: Duration,
    peak_total: Duration,
}

impl Worker {
    fn run(
        mut self,
        control_rx: Receiver<ControlMsg>,
        done_tx: Sender<ShutdownReport>,
        start_rx: Receiver<StartSignal>,
    ) {
        // Stay inactive until explicitly activated. A cancel (or a dropped
        // sender) exits immediately with no flush or persistence, so a startup
        // that fails before activation never touches profile data and leaves no
        // detached lock holder behind.
        match start_rx.recv() {
            Ok(StartSignal::Activate) => {}
            Ok(StartSignal::Cancel) | Err(_) => {
                tracing::info!("[WORKER] staged worker cancelled before activation");
                return;
            }
        }
        tracing::info!("[WORKER] processing thread started");
        loop {
            #[cfg(feature = "latency-diagnostics")]
            let iteration_started = Instant::now();
            #[cfg(feature = "latency-diagnostics")]
            let mut wait_time = Duration::ZERO;
            #[cfg(feature = "latency-diagnostics")]
            let mut segments = LoopSegments::default();

            // 1. Service any pending control messages (non-blocking).
            let mut shutdown = false;
            loop {
                match control_rx.try_recv() {
                    Ok(msg) => {
                        if self.handle_control(msg) {
                            shutdown = true;
                            break;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        shutdown = true;
                        break;
                    }
                }
            }
            if shutdown {
                break;
            }

            // 2. Obtain a batch of packets.
            #[cfg(feature = "latency-diagnostics")]
            let mut queue_remaining = 0usize;
            #[cfg(feature = "latency-diagnostics")]
            let mut capture_queue_drops = self.capture_drops.total();
            let packets = match self.receiver.as_ref() {
                Some(rx) => {
                    #[cfg(feature = "latency-diagnostics")]
                    let wait_started = Instant::now();
                    let result = rx.recv_timeout(MAX_PACKETS_PER_BATCH, RECV_TIMEOUT);
                    #[cfg(feature = "latency-diagnostics")]
                    {
                        wait_time += wait_started.elapsed();
                        capture_queue_drops = self.capture_drops.observe(rx.dropped());
                    }
                    if result.closed {
                        tracing::warn!("[WORKER] capture source closed; awaiting restart");
                        #[cfg(feature = "latency-diagnostics")]
                        {
                            capture_queue_drops = self.capture_drops.finish_source(rx.dropped());
                        }
                        self.receiver = None;
                    }
                    #[cfg(feature = "latency-diagnostics")]
                    {
                        queue_remaining = result.queue_remaining;
                    }
                    result.packets
                }
                None => {
                    // No capture: block on control so we stay responsive and
                    // still run periodic tasks, without busy-looping.
                    #[cfg(feature = "latency-diagnostics")]
                    let wait_started = Instant::now();
                    match control_rx.recv_timeout(RECV_TIMEOUT) {
                        Ok(msg) => {
                            if self.handle_control(msg) {
                                break;
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                    #[cfg(feature = "latency-diagnostics")]
                    {
                        wait_time += wait_started.elapsed();
                    }
                    Vec::new()
                }
            };
            #[cfg(feature = "latency-diagnostics")]
            let drained_at = Instant::now();
            #[cfg(feature = "latency-diagnostics")]
            let packet_latency = inspect_packet_latency(&packets, drained_at);
            #[cfg(feature = "latency-diagnostics")]
            let packet_count = packets.len();

            // 3. Run the pipeline and dispatch the resulting updates.
            let missions_changed = self.processor.expire_mission_cooldowns();
            let identity_changed = self.processor.tick_identity();
            if !packets.is_empty() {
                // Inject queue-drop count into capture metrics.
                if let Some(rx) = &self.receiver {
                    self.processor.update_queue_drops(rx.dropped());
                }

                #[cfg(feature = "latency-diagnostics")]
                let process_started = Instant::now();
                let updates = self.processor.process_available(packets);
                #[cfg(feature = "latency-diagnostics")]
                {
                    segments.process = process_started.elapsed();
                }
                let active = ActiveTab::from_u8(self.active_tab.load(Ordering::Relaxed));
                // Wake the UI only for updates the active tab (or a global widget)
                // actually displays; `suppressed` tracks irrelevant traffic so a
                // slow fallback still drains the queue and covers background tabs.
                let mut wake = false;
                let mut suppressed = false;
                #[cfg(feature = "latency-diagnostics")]
                let dispatch_started = Instant::now();
                for update in updates {
                    match update.payload {
                        UiPayload::Audio(cmd) => self.audio_tx.send(cmd),
                        payload => {
                            if payload_relevant(&payload, active) {
                                wake = true;
                            } else {
                                suppressed = true;
                            }
                            if self
                                .ui_tx
                                .send(UiUpdate {
                                    seq: update.seq,
                                    #[cfg(feature = "latency-diagnostics")]
                                    enqueued_at: None,
                                    payload,
                                })
                                .is_err()
                            {
                                // UI gone; nothing more to do.
                                return;
                            }
                        }
                    }
                }
                #[cfg(feature = "latency-diagnostics")]
                {
                    segments.dispatch = dispatch_started.elapsed();
                }
                // Scalar view state (header/status bar + generation-driven tab
                // resyncs) is always global, so any change always wakes the UI.
                #[cfg(feature = "latency-diagnostics")]
                let snapshot_started = Instant::now();
                let view_changed = self.publish_snapshot();
                #[cfg(feature = "latency-diagnostics")]
                {
                    segments.snapshot = snapshot_started.elapsed();
                }
                if wake || view_changed {
                    self.ctx.request_repaint();
                } else if suppressed {
                    self.ctx.request_repaint_after(SUPPRESSED_DRAIN_INTERVAL);
                }
            } else if missions_changed || identity_changed {
                #[cfg(feature = "latency-diagnostics")]
                let snapshot_started = Instant::now();
                self.publish_snapshot();
                #[cfg(feature = "latency-diagnostics")]
                {
                    segments.snapshot = snapshot_started.elapsed();
                }
                self.ctx.request_repaint();
            }

            // 4. Interim periodic persistence.
            #[cfg(feature = "latency-diagnostics")]
            let save_started = Instant::now();
            self.maybe_periodic_save();
            #[cfg(feature = "latency-diagnostics")]
            {
                segments.save = save_started.elapsed();
                let iteration_ended = Instant::now();
                segments.total = iteration_ended
                    .duration_since(iteration_started)
                    .saturating_sub(wait_time);
                segments.misc = segments.total.saturating_sub(
                    segments.process + segments.dispatch + segments.snapshot + segments.save,
                );
                self.latency.observe(
                    iteration_ended,
                    packet_latency,
                    packet_count,
                    queue_remaining,
                    capture_queue_drops,
                    segments,
                );
            }
        }

        // Shutdown: drain combat/loot, flush account data, close databases,
        // publish a final snapshot, then report the outcome.
        self.processor.final_drain();
        let persistence = self.processor.save_account_on_exit();
        let db_close = self.processor.close_databases();
        let report = ShutdownReport::from_results(persistence, db_close);
        self.publish_snapshot();
        tracing::info!("[WORKER] processing thread stopped: {:?}", report);
        let _ = done_tx.send(report);
    }
}

#[cfg(feature = "latency-diagnostics")]
fn inspect_packet_latency(
    packets: &[realmhound_core::capture::RawPacket],
    drained_at: Instant,
) -> PacketLatency {
    let now = chrono::Utc::now();
    let mut result = PacketLatency::default();
    for packet in packets {
        let Some(enqueued_at) = packet.enqueued_at else {
            continue;
        };
        let queue = drained_at.duration_since(enqueued_at);
        result.queue = result.queue.max(queue);
        let wall_age = now.signed_duration_since(packet.timestamp);
        let Ok(age) = wall_age.to_std() else {
            result.clock_skew = true;
            result.skew_skipped_packets += 1;
            continue;
        };
        if queue > age.saturating_add(Duration::from_millis(5)) {
            result.clock_skew = true;
            result.skew_skipped_packets += 1;
            continue;
        }
        result.age = result.age.max(age);
        result.long_age |= age > Duration::from_secs(60);
        result.pre_queue = result.pre_queue.max(age.saturating_sub(queue));
    }
    result
}

#[cfg(feature = "latency-diagnostics")]
impl LatencyDiagnostics {
    fn observe(
        &mut self,
        now: Instant,
        packet: PacketLatency,
        packet_count: usize,
        queue_remaining: usize,
        capture_drops: u64,
        segments: LoopSegments,
    ) {
        if packet.clock_skew && !self.clock_skew_logged {
            self.clock_skew_logged = true;
            tracing::warn!(
                "[LATENCY][PIPELINE] clock_skew invalid pcap/enqueue clock relationship; \
                 pre_queue_age_omitted=true skew_skipped_packets={}",
                packet.skew_skipped_packets
            );
        }

        let sustained_remaining = if queue_remaining > MAX_PACKETS_PER_BATCH {
            let since = self.large_remaining_since.get_or_insert(now);
            now.duration_since(*since) >= Duration::from_secs(1)
        } else {
            self.large_remaining_since = None;
            false
        };
        let capture_drop_delta = capture_drops.saturating_sub(self.previous_capture_drops);
        if capture_drops < self.previous_capture_drops {
            self.capture_drop_episode_active = false;
        }
        self.previous_capture_drops = capture_drops;
        let first_capture_drop_increase =
            capture_drop_delta > 0 && !self.capture_drop_episode_active;
        if capture_drop_delta > 0 {
            self.capture_drop_episode_active = true;
        } else if self.backlog.is_none() {
            self.capture_drop_episode_active = false;
        }
        if let Some(health) = self.health.record(
            now,
            packet_count,
            &packet,
            queue_remaining,
            capture_drop_delta,
            capture_drops,
            &segments,
        ) {
            tracing::info!(
                "[LATENCY][PIPELINE] health window_ms={} packets={} batches={} \
                 peak_pre_queue_ms={} peak_queue_ms={} peak_packet_age_ms={} \
                 peak_queue_remaining={} capture_queue_drop_delta={} \
                 capture_queue_drops={} skew_skipped_packets={} peak_process_ms={} \
                 peak_dispatch_ms={} peak_snapshot_ms={} peak_save_ms={} peak_misc_ms={} \
                 peak_loop_non_wait_ms={}",
                health.window.as_millis(),
                health.packets,
                health.batches,
                health.peak_pre_queue.as_millis(),
                health.peak_queue.as_millis(),
                health.peak_age.as_millis(),
                health.peak_remaining,
                health.capture_drop_delta,
                health.capture_drops,
                health.skew_skipped_packets,
                health.peak_process.as_millis(),
                health.peak_dispatch.as_millis(),
                health.peak_snapshot.as_millis(),
                health.peak_save.as_millis(),
                health.peak_misc.as_millis(),
                health.peak_total.as_millis()
            );
        }
        let diagnosed = packet.age >= PIPELINE_WARN_AGE
            || packet.queue >= PIPELINE_WARN_AGE
            || segments.total >= PIPELINE_WARN_WORK
            || sustained_remaining
            || capture_drop_delta > 0;

        if diagnosed {
            let backlog = self.backlog.get_or_insert_with(|| Backlog {
                started: now,
                peak_pre_queue: Duration::ZERO,
                peak_queue: Duration::ZERO,
                peak_age: Duration::ZERO,
                clock_skew_seen: false,
                skew_skipped_packets: 0,
                peak_remaining: 0,
                peak_drops: 0,
                capture_drop_delta: 0,
                slowest_process: Duration::ZERO,
                slowest_dispatch: Duration::ZERO,
                slowest_snapshot: Duration::ZERO,
                slowest_save: Duration::ZERO,
                slowest_misc: Duration::ZERO,
                slowest_loop: Duration::ZERO,
            });
            backlog.peak_pre_queue = backlog.peak_pre_queue.max(packet.pre_queue);
            backlog.peak_queue = backlog.peak_queue.max(packet.queue);
            backlog.peak_age = backlog.peak_age.max(packet.age);
            backlog.clock_skew_seen |= packet.clock_skew;
            backlog.skew_skipped_packets = backlog
                .skew_skipped_packets
                .saturating_add(packet.skew_skipped_packets);
            backlog.peak_remaining = backlog.peak_remaining.max(queue_remaining);
            backlog.peak_drops = backlog.peak_drops.max(capture_drops);
            backlog.capture_drop_delta = backlog
                .capture_drop_delta
                .saturating_add(capture_drop_delta);
            backlog.slowest_process = backlog.slowest_process.max(segments.process);
            backlog.slowest_dispatch = backlog.slowest_dispatch.max(segments.dispatch);
            backlog.slowest_snapshot = backlog.slowest_snapshot.max(segments.snapshot);
            backlog.slowest_save = backlog.slowest_save.max(segments.save);
            backlog.slowest_misc = backlog.slowest_misc.max(segments.misc);
            backlog.slowest_loop = backlog.slowest_loop.max(segments.total);

            let extreme =
                packet.age >= PIPELINE_EXTREME_AGE || segments.total >= PIPELINE_EXTREME_WORK;
            let entered_extreme = extreme && !self.extreme_active;
            self.extreme_active = extreme;
            let extreme_transition_warn = entered_extreme
                && self
                    .last_extreme_transition_warn
                    .is_none_or(|last| now.duration_since(last) >= Duration::from_millis(500));
            if extreme_transition_warn {
                self.last_extreme_transition_warn = Some(now);
            }
            if extreme_transition_warn
                || first_capture_drop_increase
                || self
                    .last_warn
                    .is_none_or(|last| now.duration_since(last) >= PIPELINE_WARN_THROTTLE)
            {
                self.last_warn = Some(now);
                tracing::warn!(
                    "[LATENCY][PIPELINE] pre_queue_ms={} queue_ms={} packet_age_ms={} \
                         queue_remaining={} capture_queue_drop_delta={} capture_queue_drops={} \
                         process_ms={} dispatch_ms={} snapshot_ms={} save_ms={} misc_ms={} \
                         loop_non_wait_ms={} sustained_backlog={} extreme={} long_age={} \
                         clock_skew={} skew_skipped_packets={}",
                    packet.pre_queue.as_millis(),
                    packet.queue.as_millis(),
                    packet.age.as_millis(),
                    queue_remaining,
                    capture_drop_delta,
                    capture_drops,
                    segments.process.as_millis(),
                    segments.dispatch.as_millis(),
                    segments.snapshot.as_millis(),
                    segments.save.as_millis(),
                    segments.misc.as_millis(),
                    segments.total.as_millis(),
                    sustained_remaining,
                    extreme,
                    packet.long_age,
                    packet.clock_skew,
                    packet.skew_skipped_packets
                );
            }
        } else if let Some(backlog) = self.backlog.take() {
            self.extreme_active = false;
            self.capture_drop_episode_active = false;
            tracing::info!(
                "[LATENCY][PIPELINE] catch_up backlog_duration_ms={} peak_pre_queue_ms={} \
                     peak_queue_ms={} peak_packet_age_ms={} peak_queue_remaining={} \
                     capture_queue_drop_delta={} capture_queue_drops={} slowest_process_ms={} \
                     slowest_dispatch_ms={} slowest_snapshot_ms={} slowest_save_ms={} \
                     slowest_misc_ms={} slowest_loop_non_wait_ms={} clock_skew={} \
                     skew_skipped_packets={}",
                now.duration_since(backlog.started).as_millis(),
                backlog.peak_pre_queue.as_millis(),
                backlog.peak_queue.as_millis(),
                backlog.peak_age.as_millis(),
                backlog.peak_remaining,
                backlog.capture_drop_delta,
                backlog.peak_drops,
                backlog.slowest_process.as_millis(),
                backlog.slowest_dispatch.as_millis(),
                backlog.slowest_snapshot.as_millis(),
                backlog.slowest_save.as_millis(),
                backlog.slowest_misc.as_millis(),
                backlog.slowest_loop.as_millis(),
                backlog.clock_skew_seen,
                backlog.skew_skipped_packets
            );
        }
    }
}

#[cfg(feature = "latency-diagnostics")]
impl PipelineHealth {
    fn record(
        &mut self,
        now: Instant,
        packet_count: usize,
        packet: &PacketLatency,
        queue_remaining: usize,
        capture_drop_delta: u64,
        capture_drops: u64,
        segments: &LoopSegments,
    ) -> Option<PipelineHealthSummary> {
        if packet_count == 0 && capture_drop_delta == 0 {
            return None;
        }
        let started = *self.started.get_or_insert(now);
        self.packets = self.packets.saturating_add(packet_count as u64);
        self.batches = self.batches.saturating_add(u64::from(packet_count > 0));
        self.peak_pre_queue = self.peak_pre_queue.max(packet.pre_queue);
        self.peak_queue = self.peak_queue.max(packet.queue);
        self.peak_age = self.peak_age.max(packet.age);
        self.peak_remaining = self.peak_remaining.max(queue_remaining);
        self.capture_drop_delta = self.capture_drop_delta.saturating_add(capture_drop_delta);
        self.capture_drops = capture_drops;
        self.skew_skipped_packets = self
            .skew_skipped_packets
            .saturating_add(packet.skew_skipped_packets);
        self.peak_process = self.peak_process.max(segments.process);
        self.peak_dispatch = self.peak_dispatch.max(segments.dispatch);
        self.peak_snapshot = self.peak_snapshot.max(segments.snapshot);
        self.peak_save = self.peak_save.max(segments.save);
        self.peak_misc = self.peak_misc.max(segments.misc);
        self.peak_total = self.peak_total.max(segments.total);

        let window = now.duration_since(started);
        if window < HEALTH_MIN_INTERVAL {
            return None;
        }
        let summary = PipelineHealthSummary {
            window,
            packets: self.packets,
            batches: self.batches,
            peak_pre_queue: self.peak_pre_queue,
            peak_queue: self.peak_queue,
            peak_age: self.peak_age,
            peak_remaining: self.peak_remaining,
            capture_drop_delta: self.capture_drop_delta,
            capture_drops: self.capture_drops,
            skew_skipped_packets: self.skew_skipped_packets,
            peak_process: self.peak_process,
            peak_dispatch: self.peak_dispatch,
            peak_snapshot: self.peak_snapshot,
            peak_save: self.peak_save,
            peak_misc: self.peak_misc,
            peak_total: self.peak_total,
        };
        *self = Self::default();
        Some(summary)
    }
}

impl Worker {
    /// Handle a control message. Returns `true` if the worker should shut down.
    fn handle_control(&mut self, msg: ControlMsg) -> bool {
        match msg {
            ControlMsg::Shutdown => return true,
            ControlMsg::StartCapture(receiver) => {
                self.processor.on_capture_start();
                #[cfg(feature = "latency-diagnostics")]
                if let Some(previous) = self.receiver.take() {
                    self.capture_drops.finish_source(previous.dropped());
                }
                self.receiver = Some(receiver);
                // Reset publishes so the UI re-syncs after a capture restart.
                self.publish_snapshot();
                self.ctx.request_repaint();
            }
            ControlMsg::StopCapture => {
                #[cfg(feature = "latency-diagnostics")]
                if let Some(receiver) = self.receiver.take() {
                    self.capture_drops.finish_source(receiver.dropped());
                }
                #[cfg(not(feature = "latency-diagnostics"))]
                {
                    self.receiver = None;
                }
            }
            other => {
                self.processor.apply_control(other);
                let view_changed = self.publish_snapshot();
                if view_changed {
                    self.ctx.request_repaint();
                }
            }
        }
        false
    }

    /// Periodic account save, mirroring the App's prior 60s/dirty policy.
    fn maybe_periodic_save(&mut self) {
        if self.last_save.elapsed() >= SAVE_INTERVAL {
            if self.processor.save_account_if_dirty() {
                self.last_save = Instant::now();
            }
        }
    }

    /// Publish the latest model state into the shared snapshot, cloning heavy
    /// fields only when their generation changed. Returns whether the scalar
    /// view state changed (so the caller can request a repaint).
    fn publish_snapshot(&mut self) -> bool {
        let view = self.processor.view_state();
        let char_gen = view.char_view_gen;
        let account_gen = view.account_generation;
        let vault_gen = view.vault_view_gen;
        let live_char_id = self.processor.live_char_id();
        let main_connection = self.processor.main_connection();

        let account_changed = char_gen != self.pub_char_gen || account_gen != self.pub_account_gen;
        let vault_changed = vault_gen != self.pub_vault_gen;
        let mission_gen = view.mission_view_gen;
        let mission_changed = mission_gen != self.pub_mission_gen;
        let view_changed = self.pub_view.as_ref() != Some(&view);

        if let Ok(mut snap) = self.snapshot.lock() {
            snap.view = view.clone();
            snap.live_char_id = live_char_id;
            snap.main_connection = main_connection;
            if account_changed {
                snap.account_data = self.processor.account_data().clone();
            }
            if vault_changed {
                snap.live_vault_data = self.processor.live_vault_data();
            }
            if mission_changed {
                snap.mission_view = self.processor.mission_view();
            }
        }

        self.pub_char_gen = char_gen;
        self.pub_account_gen = account_gen;
        self.pub_vault_gen = vault_gen;
        self.pub_mission_gen = mission_gen;
        self.pub_view = Some(view);
        view_changed
    }
}

/// Whether a produced [`UiPayload`] should wake the UI given the current
/// `active` tab. Payloads that feed always-visible global widgets (the party
/// status bar) or carry side effects return `true` regardless of tab; the rest
/// wake only the tabs whose panels render them. `Audio` is routed to the audio
/// thread before this decision and never reaches here.
fn payload_relevant(payload: &UiPayload, active: ActiveTab) -> bool {
    use ActiveTab::*;
    match payload {
        // Global: drive the always-visible status bar or have deferred-unsafe
        // side effects (token persistence + one-time exalt fetch).
        UiPayload::PartyMemberLeft(_)
        | UiPayload::PartyLocalLeaderStatus(_)
        | UiPayload::PartyLocalPlayerName(_)
        | UiPayload::PartyCleared
        | UiPayload::AccessTokenCaptured(_) => true,

        // Every GameEvent is broadcast here. Party-membership variants also feed
        // the global status bar, so they always wake; the rest wake only the
        // tabs that render live game events.
        UiPayload::Broadcast(event) => {
            is_global_event(event)
                || matches!(active, LiveFeed | Quests | Party | Vault | CombatHistory)
        }

        UiPayload::Chat(_) => matches!(active, Chat | LiveFeed),
        UiPayload::PushLoot(_) => matches!(active, LiveFeed | LootHistory | TrophyHall),
        UiPayload::LootBatchAdded => matches!(active, LootHistory | TrophyHall),
        UiPayload::UpdateVaultPetSlot { .. } => matches!(active, Vault | PetYard),
        UiPayload::SetVaultActivePet { .. } => matches!(active, Vault | PetYard),

        UiPayload::BossCall { .. }
        | UiPayload::KeyperEvent { .. }
        | UiPayload::SetServerName(_)
        | UiPayload::AddEncounter { .. }
        | UiPayload::RemoveEncounter(_)
        | UiPayload::RealmClosed
        | UiPayload::KeyPop { .. }
        | UiPayload::AreaUnlock { .. }
        | UiPayload::DungeonTimerFrozen { .. }
        | UiPayload::OryxLagWarning => matches!(active, LiveFeed),

        // Handled by the audio thread before this point.
        UiPayload::Audio(_) => false,
    }
}

/// Broadcast events that mutate the always-visible party status bar (leader
/// crown + member/watchlist counts), so they must wake any active tab.
fn is_global_event(event: &GameEvent) -> bool {
    matches!(
        event,
        GameEvent::PartyListReceived(_)
            | GameEvent::PartyMemberJoined(_)
            | GameEvent::PartyJoinRequestResponse(_)
            | GameEvent::PartyActionReceived { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn non_party_broadcast() -> UiPayload {
        UiPayload::Broadcast(GameEvent::EntityHit { target_id: 1 })
    }

    fn party_broadcast() -> UiPayload {
        UiPayload::Broadcast(GameEvent::PartyActionReceived {
            player_id: 0,
            action_id: 0,
        })
    }

    #[test]
    fn broadcast_wakes_only_dynamic_game_tabs() {
        let p = non_party_broadcast();
        for tab in [
            ActiveTab::LiveFeed,
            ActiveTab::Quests,
            ActiveTab::Party,
            ActiveTab::Vault,
            ActiveTab::CombatHistory,
        ] {
            assert!(
                payload_relevant(&p, tab),
                "{:?} should wake on broadcast",
                tab
            );
        }
        // Expensive static tabs must sleep through unrelated broadcast traffic.
        for tab in [
            ActiveTab::Treasury,
            ActiveTab::TrophyHall,
            ActiveTab::Characters,
            ActiveTab::Exaltations,
            ActiveTab::LootHistory,
            ActiveTab::Chat,
        ] {
            assert!(
                !payload_relevant(&p, tab),
                "{:?} should sleep on broadcast",
                tab
            );
        }
    }

    #[test]
    fn party_membership_broadcast_is_global() {
        // Feeds the always-visible status bar, so it wakes even a static tab.
        assert!(payload_relevant(&party_broadcast(), ActiveTab::Treasury));
        assert!(payload_relevant(&party_broadcast(), ActiveTab::Characters));
    }

    #[test]
    fn party_status_payloads_are_global() {
        for p in [
            UiPayload::PartyMemberLeft("x".into()),
            UiPayload::PartyLocalLeaderStatus(true),
        ] {
            assert!(payload_relevant(&p, ActiveTab::Treasury));
            assert!(payload_relevant(&p, ActiveTab::TrophyHall));
        }
    }

    #[test]
    fn access_token_capture_is_global() {
        let p = UiPayload::AccessTokenCaptured("tok".into());
        assert!(payload_relevant(&p, ActiveTab::Treasury));
    }

    #[test]
    fn loot_payloads_wake_loot_and_trophy_tabs() {
        let p = UiPayload::LootBatchAdded;
        assert!(payload_relevant(&p, ActiveTab::LootHistory));
        assert!(payload_relevant(&p, ActiveTab::TrophyHall));
        assert!(!payload_relevant(&p, ActiveTab::Treasury));
        assert!(!payload_relevant(&p, ActiveTab::LiveFeed));
    }

    #[test]
    fn live_feed_only_payloads_scope_to_live_feed() {
        for p in [
            UiPayload::RealmClosed,
            UiPayload::OryxLagWarning,
            UiPayload::RemoveEncounter(1),
            UiPayload::SetServerName("s".into()),
        ] {
            assert!(payload_relevant(&p, ActiveTab::LiveFeed));
            assert!(!payload_relevant(&p, ActiveTab::Treasury));
            assert!(!payload_relevant(&p, ActiveTab::Party));
        }
    }

    #[test]
    fn vault_pet_slot_scopes_to_vault() {
        let p = UiPayload::UpdateVaultPetSlot {
            pet_instance_id: 1,
            seasonal: false,
            slot: 0,
            item_id: 2,
            stack_count: 1,
        };
        assert!(payload_relevant(&p, ActiveTab::Vault));
        assert!(!payload_relevant(&p, ActiveTab::LiveFeed));
    }

    #[cfg(feature = "latency-diagnostics")]
    #[test]
    fn packet_latency_separates_pre_queue_and_queue_time() {
        let drained_at = Instant::now();
        let packet = realmhound_core::capture::RawPacket {
            timestamp: chrono::Utc::now() - chrono::Duration::milliseconds(700),
            enqueued_at: Some(drained_at - Duration::from_millis(200)),
            data: Vec::new(),
        };
        let latency = inspect_packet_latency(&[packet], drained_at);
        assert!(latency.pre_queue >= Duration::from_millis(450));
        assert_eq!(latency.queue, Duration::from_millis(200));
        assert!(!latency.clock_skew);
    }

    #[cfg(feature = "latency-diagnostics")]
    #[test]
    fn packet_latency_flags_future_capture_clock() {
        let drained_at = Instant::now();
        let packet = realmhound_core::capture::RawPacket {
            timestamp: chrono::Utc::now() + chrono::Duration::seconds(1),
            enqueued_at: Some(drained_at),
            data: Vec::new(),
        };
        let latency = inspect_packet_latency(&[packet], drained_at);
        assert!(latency.clock_skew);
        assert_eq!(latency.skew_skipped_packets, 1);
    }

    #[cfg(feature = "latency-diagnostics")]
    #[test]
    fn packet_latency_preserves_long_packet_age() {
        let drained_at = Instant::now();
        let packet = realmhound_core::capture::RawPacket {
            timestamp: chrono::Utc::now() - chrono::Duration::seconds(90),
            enqueued_at: Some(drained_at - Duration::from_millis(200)),
            data: Vec::new(),
        };
        let latency = inspect_packet_latency(&[packet], drained_at);
        assert!(latency.age >= Duration::from_secs(89));
        assert!(latency.long_age);
        assert!(!latency.clock_skew);
    }

    #[cfg(feature = "latency-diagnostics")]
    #[test]
    fn sustained_extreme_pipeline_latency_is_throttled() {
        let now = Instant::now();
        let mut diagnostics = LatencyDiagnostics::default();
        diagnostics.observe(
            now,
            PacketLatency {
                age: Duration::from_secs(3),
                ..PacketLatency::default()
            },
            1,
            0,
            0,
            LoopSegments::default(),
        );
        let first_warn = diagnostics.last_warn;
        diagnostics.observe(
            now + Duration::from_millis(100),
            PacketLatency {
                age: Duration::from_secs(4),
                ..PacketLatency::default()
            },
            1,
            0,
            0,
            LoopSegments::default(),
        );
        assert_eq!(diagnostics.last_warn, first_warn);
    }

    #[cfg(feature = "latency-diagnostics")]
    #[test]
    fn extreme_transition_warning_has_minimum_floor() {
        let now = Instant::now();
        let extreme = || PacketLatency {
            age: Duration::from_secs(3),
            ..PacketLatency::default()
        };
        let ordinary = || PacketLatency {
            age: Duration::from_millis(600),
            ..PacketLatency::default()
        };
        let mut diagnostics = LatencyDiagnostics::default();
        diagnostics.observe(now, extreme(), 1, 0, 0, LoopSegments::default());
        diagnostics.observe(
            now + Duration::from_millis(50),
            ordinary(),
            1,
            0,
            0,
            LoopSegments::default(),
        );
        diagnostics.observe(
            now + Duration::from_millis(100),
            extreme(),
            1,
            0,
            0,
            LoopSegments::default(),
        );
        assert_eq!(diagnostics.last_warn, Some(now));

        diagnostics.observe(
            now + Duration::from_millis(550),
            ordinary(),
            1,
            0,
            0,
            LoopSegments::default(),
        );
        diagnostics.observe(
            now + Duration::from_millis(600),
            extreme(),
            1,
            0,
            0,
            LoopSegments::default(),
        );
        assert_eq!(
            diagnostics.last_warn,
            Some(now + Duration::from_millis(600))
        );
    }

    #[cfg(feature = "latency-diagnostics")]
    #[test]
    fn capture_drop_increases_start_diagnostic_and_accumulate() {
        let now = Instant::now();
        let mut diagnostics = LatencyDiagnostics::default();
        diagnostics.observe(
            now,
            PacketLatency::default(),
            0,
            0,
            3,
            LoopSegments::default(),
        );
        assert_eq!(diagnostics.last_warn, Some(now));
        assert_eq!(
            diagnostics
                .backlog
                .as_ref()
                .map(|backlog| backlog.capture_drop_delta),
            Some(3)
        );

        diagnostics.observe(
            now + Duration::from_millis(100),
            PacketLatency::default(),
            0,
            0,
            5,
            LoopSegments::default(),
        );
        assert_eq!(diagnostics.last_warn, Some(now));
        assert_eq!(
            diagnostics
                .backlog
                .as_ref()
                .map(|backlog| backlog.capture_drop_delta),
            Some(5)
        );
    }

    #[cfg(feature = "latency-diagnostics")]
    #[test]
    fn pipeline_health_emits_actual_window_and_resets() {
        let now = Instant::now();
        let mut health = PipelineHealth::default();
        assert!(health
            .record(
                now,
                0,
                &PacketLatency::default(),
                0,
                0,
                0,
                &LoopSegments::default(),
            )
            .is_none());
        assert!(health
            .record(
                now,
                10,
                &PacketLatency {
                    pre_queue: Duration::from_millis(20),
                    queue: Duration::from_millis(30),
                    age: Duration::from_millis(50),
                    ..PacketLatency::default()
                },
                4,
                1,
                7,
                &LoopSegments {
                    process: Duration::from_millis(8),
                    total: Duration::from_millis(12),
                    ..LoopSegments::default()
                },
            )
            .is_none());
        let summary = health
            .record(
                now + Duration::from_secs(35),
                5,
                &PacketLatency::default(),
                1,
                2,
                9,
                &LoopSegments::default(),
            )
            .unwrap();
        assert_eq!(summary.window, Duration::from_secs(35));
        assert_eq!(summary.packets, 15);
        assert_eq!(summary.capture_drop_delta, 3);
        assert_eq!(summary.peak_queue, Duration::from_millis(30));
        assert!(health.started.is_none());
        assert_eq!(health.packets, 0);
    }

    #[cfg(feature = "latency-diagnostics")]
    #[test]
    fn capture_drop_totals_remain_monotonic_across_source_restart() {
        let mut drops = CaptureDropTotals::default();
        assert_eq!(drops.observe(3), 3);
        assert_eq!(drops.observe(7), 7);
        assert_eq!(drops.observe(2), 9);
        assert_eq!(drops.observe(5), 12);
    }

    #[cfg(feature = "latency-diagnostics")]
    #[test]
    fn capture_drop_totals_roll_final_source_count_into_base() {
        let mut drops = CaptureDropTotals::default();
        assert_eq!(drops.observe(4), 4);
        assert_eq!(drops.finish_source(6), 6);
        assert_eq!(drops.observe(2), 8);
        assert_eq!(drops.finish_source(2), 8);
    }
}

#[cfg(test)]
mod staging_tests {
    use std::sync::{Arc, RwLock};

    use realmhound_core::account::{
        AccountContext, InMemoryCredentialStore, StartupResolution, StartupResolver,
    };
    use realmhound_core::storage::StorageRoot;
    use realmhound_core::vault::AccountData;
    use realmhound_core::Settings;

    use crate::audio::AudioSender;
    use crate::processing::processor::PacketProcessor;

    use super::*;

    /// Resolve a selected profile over a fresh temp root seeded with a minimal
    /// known flat layout, returning its immutable account context.
    fn selected_context(root: &StorageRoot) -> Arc<AccountContext> {
        std::fs::write(
            root.path().join("settings.json"),
            br#"{"account":{"account_id":"ACCT-STAGE","account_name":"Hero"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.path().join("account_data.json"),
            serde_json::to_vec(&AccountData::new()).unwrap(),
        )
        .unwrap();
        let credentials = Arc::new(InMemoryCredentialStore::new());
        match StartupResolver::new(root.clone(), None, credentials).resolve() {
            StartupResolution::Selected(selected) => Arc::new(selected.into_context()),
            _ => panic!("expected a selected startup for the seeded known layout"),
        }
    }

    fn processor(account: &AccountContext) -> PacketProcessor {
        let persistence = account.persistence();
        PacketProcessor::new_selected(
            Arc::new(RwLock::new(Settings::default())),
            AccountData::new(),
            persistence.account_data_repository(),
            persistence.loot_database().to_path_buf(),
            persistence.combat_database().to_path_buf(),
            Some(account.account_id().to_string()),
            None,
        )
        .expect("writers open for the selected profile")
    }

    /// Write a distinctive marker snapshot so a later persist is observable.
    fn write_marker(account: &AccountContext) {
        let mut marker = AccountData::new();
        marker.generation = 42;
        std::fs::write(
            account.persistence().account_data(),
            serde_json::to_vec(&marker).unwrap(),
        )
        .unwrap();
    }

    fn stored_generation(account: &AccountContext) -> u64 {
        let bytes = std::fs::read(account.persistence().account_data()).unwrap();
        serde_json::from_slice::<AccountData>(&bytes)
            .unwrap()
            .generation
    }

    #[test]
    fn cancelled_staged_worker_never_persists() {
        let temp = tempfile::tempdir().unwrap();
        let root = StorageRoot::from_path(temp.path()).unwrap();
        let account = selected_context(&root);
        write_marker(&account);

        let worker = spawn(
            processor(&account),
            egui::Context::default(),
            AudioSender::disconnected(),
            Arc::new(AtomicU8::new(0)),
            account.clone(),
        )
        .expect("worker thread spawns");

        // Cancel before activation: the worker exits without any flush, so the
        // marker snapshot on disk is untouched (no empty overwrite).
        worker.cancel_staged();
        assert_eq!(stored_generation(&account), 42);
    }

    #[test]
    fn activated_worker_persists_on_shutdown() {
        let temp = tempfile::tempdir().unwrap();
        let root = StorageRoot::from_path(temp.path()).unwrap();
        let account = selected_context(&root);
        write_marker(&account);

        let mut worker = spawn(
            processor(&account),
            egui::Context::default(),
            AudioSender::disconnected(),
            Arc::new(AtomicU8::new(0)),
            account.clone(),
        )
        .expect("worker thread spawns");

        // Once activated, a clean shutdown flushes the (empty) in-memory
        // snapshot, overwriting the marker -- proving activation gates persistence.
        worker.activate();
        worker.shutdown(std::time::Duration::from_secs(5));
        assert_eq!(stored_generation(&account), 0);
    }
}

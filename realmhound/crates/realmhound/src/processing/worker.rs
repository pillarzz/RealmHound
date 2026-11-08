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

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use eframe::egui;
use realmhound_core::account::AccountContext;
use realmhound_core::capture::PacketReceiver;
use realmhound_core::watchlist::{PlayerStatus, Watchlist};
use realmhound_core::{GameEvent, Settings};

use crate::audio::AudioSender;

use super::chat_log::ChatLogWriter;
use super::contract::{
    ControlMsg, LiveEventView, ModelSnapshot, SelectedIdentity, ShutdownReport, UiPayload, UiUpdate,
};
use super::health::{CaptureState, HealthCounters};
use super::processor::PacketProcessor;
use super::ui_channel::{self, UiReceiver, UiSender};

/// How long the worker blocks waiting for packets before looping to service
/// control messages and periodic tasks.
const RECV_TIMEOUT: Duration = Duration::from_millis(100);
/// Max packets drained per pipeline batch (keeps batches bounded).
const MAX_PACKETS_PER_BATCH: usize = 256;
/// Periodic account-data save interval (interim S2 persistence).
const SAVE_INTERVAL: Duration = Duration::from_secs(60);
/// Background health summary interval, independent of UI painting.
const HEALTH_LOG_INTERVAL: Duration = Duration::from_secs(30);
/// Target maximum repaint rate from worker traffic (~10 Hz). The trailing
/// `request_repaint_after` guarantees the last update is still painted.
const WAKE_INTERVAL: Duration = Duration::from_millis(100);
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

/// Automatic dungeon callout currently on the clipboard.
struct AutoCallout {
    text: String,
    expires_at: Instant,
}

struct WakeThrottle {
    last: Option<Instant>,
    /// Skip wakes entirely while the UI cannot paint (minimized).
    demand: bool,
}

impl Default for WakeThrottle {
    fn default() -> Self {
        Self {
            last: None,
            demand: true,
        }
    }
}

impl WakeThrottle {
    /// Coalesce worker wake-ups to [`WAKE_INTERVAL`], scheduling the trailing
    /// repaint instead of dropping it.
    fn request(&mut self, ctx: &egui::Context) {
        if !self.demand {
            return;
        }
        match self.next_delay(Instant::now()) {
            Some(delay) => ctx.request_repaint_after(delay),
            None => ctx.request_repaint(),
        }
    }

    fn set_demand(&mut self, demand: bool) {
        if demand && !self.demand {
            // Wake as soon as the next update arrives rather than waiting out
            // a throttle window that started before the window was restored.
            self.last = None;
        }
        self.demand = demand;
    }

    fn next_delay(&mut self, now: Instant) -> Option<Duration> {
        match self.last {
            Some(last) if now.saturating_duration_since(last) < WAKE_INTERVAL => {
                Some(WAKE_INTERVAL - now.saturating_duration_since(last))
            }
            _ => {
                self.last = Some(now);
                None
            }
        }
    }
}

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
    /// Shared latest-value model snapshot (pointer swap, immutable).
    pub snapshot: Arc<Mutex<Arc<ModelSnapshot>>>,
    /// Always-on health counters shared with the UI.
    pub health: Arc<HealthCounters>,
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
    account: Arc<AccountContext>,
) -> std::io::Result<WorkerHandle> {
    processor.set_selected_scope(account.account_key(), account.account_id().clone());

    let (control_tx, control_rx) = channel::<ControlMsg>();
    let (start_tx, start_rx) = channel::<StartSignal>();
    let (ui_tx, ui_rx) = ui_channel::channel(ui_channel::DEFAULT_BOUND);
    let (done_tx, done_rx) = channel::<ShutdownReport>();
    let health = Arc::new(HealthCounters::new());
    let settings = processor.settings_handle();
    let watchlist = Watchlist::load_from(
        account.storage_root().path().join("watchlist.txt"),
        Some(account.persistence().watchlist_detections().to_path_buf()),
    );
    let chat_log = ChatLogWriter::new(account.persistence().chat_logs().to_path_buf());

    let selected_identity =
        SelectedIdentity::new(account.account_key(), account.account_id().clone());
    let snapshot = Arc::new(Mutex::new(Arc::new(ModelSnapshot::new(
        selected_identity,
        processor.view_state(),
        processor.account_data().clone(),
        processor.live_vault_data(),
        processor.live_char_id(),
    ))));

    let worker = Worker {
        processor,
        ui_tx,
        audio_tx,
        snapshot: snapshot.clone(),
        health: health.clone(),
        chat_log,
        settings,
        watchlist,
        auto_callout: None,
        exalts_fetched: false,
        wake: WakeThrottle::default(),
        ctx,
        account,
        receiver: None,
        #[cfg(feature = "latency-diagnostics")]
        capture_drops: CaptureDropTotals::default(),
        last_save: Instant::now(),
        last_health_log: Instant::now(),
        next_maintenance: None,
        maintenance_history_dirty: false,
        next_maintenance_refresh: Instant::now(),
        pub_char_gen: u64::MAX,
        pub_account_gen: u64::MAX,
        pub_vault_gen: u64::MAX,
        pub_view: None,
        pub_mission_gen: u64::MAX,
        pub_token_gen: u64::MAX,
        pub_party_rev: u64::MAX,
        pub_encounter_rev: u64::MAX,
        pub_quest_rev: u64::MAX,
        pub_live_event_rev: u64::MAX,
        pub_feed_rev: u64::MAX,
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
        health,
        shutdown_done: done_rx,
        join: Some(join),
    })
}

struct Worker {
    processor: PacketProcessor,
    ui_tx: UiSender,
    audio_tx: AudioSender,
    snapshot: Arc<Mutex<Arc<ModelSnapshot>>>,
    /// Always-on health counters shared with the UI.
    health: Arc<HealthCounters>,
    /// Worker-owned chat persistence (independent of UI frames).
    chat_log: ChatLogWriter,
    /// Shared settings for worker-side side effects.
    settings: Arc<RwLock<Settings>>,
    /// Watchlist snapshot for worker-side kick/join alerts.
    watchlist: Watchlist,
    /// Automatic dungeon callout currently on the clipboard.
    auto_callout: Option<AutoCallout>,
    /// One-shot exalt stats fetch, triggered by the first released token.
    exalts_fetched: bool,
    /// Coalesced repaint requests from worker traffic.
    wake: WakeThrottle,
    ctx: egui::Context,
    /// Shared immutable account context. Held so the exclusive profile lock
    /// inside it lives as long as the worker (including after a detached
    /// shutdown), keeping the selected profile owned by this process.
    #[allow(dead_code)]
    account: Arc<AccountContext>,
    receiver: Option<PacketReceiver>,
    #[cfg(feature = "latency-diagnostics")]
    capture_drops: CaptureDropTotals,
    last_save: Instant,
    last_health_log: Instant,
    next_maintenance: Option<Instant>,
    maintenance_history_dirty: bool,
    next_maintenance_refresh: Instant,
    pub_char_gen: u64,
    pub_account_gen: u64,
    pub_vault_gen: u64,
    pub_view: Option<super::contract::ViewState>,
    pub_mission_gen: u64,
    pub_token_gen: u64,
    pub_party_rev: u64,
    pub_encounter_rev: u64,
    pub_quest_rev: u64,
    pub_live_event_rev: u64,
    pub_feed_rev: u64,
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
        let mut maintenance_log = realmhound_core::maintenance::MaintenanceLog::default();
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
            let mut queue_remaining = 0usize;
            #[cfg(feature = "latency-diagnostics")]
            let mut capture_queue_drops = self.capture_drops.total();
            let mut capture_closed = false;
            let packets = match self.receiver.as_ref() {
                Some(rx) => {
                    #[cfg(feature = "latency-diagnostics")]
                    let wait_started = Instant::now();
                    let result = rx.recv_timeout(MAX_PACKETS_PER_BATCH, RECV_TIMEOUT);
                    // Read the counter before borrowing self mutably below.
                    #[cfg(feature = "latency-diagnostics")]
                    let dropped_now = rx.dropped();
                    #[cfg(feature = "latency-diagnostics")]
                    {
                        wait_time += wait_started.elapsed();
                        capture_queue_drops = self.capture_drops.observe(dropped_now);
                    }
                    if result.closed {
                        tracing::warn!("[WORKER] capture source closed; awaiting restart");
                        self.health.set_capture_state(CaptureState::Closed);
                        self.release_auto_callout();
                        capture_closed = true;
                        #[cfg(feature = "latency-diagnostics")]
                        {
                            capture_queue_drops = self.capture_drops.finish_source(dropped_now);
                        }
                        self.receiver = None;
                    }
                    queue_remaining = result.queue_remaining;
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
            if !packets.is_empty() || capture_closed || self.processor.has_pending_updates() {
                // Inject queue-drop count into capture metrics.
                if let Some(rx) = &self.receiver {
                    self.processor.update_queue_drops(rx.dropped());
                }

                #[cfg(feature = "latency-diagnostics")]
                let process_started = Instant::now();
                let mut updates = self.processor.process_available(packets);
                if capture_closed {
                    self.processor.on_capture_stop();
                    updates.extend(self.processor.process_available(Vec::new()));
                }
                #[cfg(feature = "latency-diagnostics")]
                {
                    segments.process = process_started.elapsed();
                }
                // Wake hints are paint-only: every UI update wakes the throttled
                // UI, regardless of the active tab.
                let mut had_ui_updates = false;
                #[cfg(feature = "latency-diagnostics")]
                let dispatch_started = Instant::now();
                for update in updates {
                    match update.payload {
                        UiPayload::Audio(cmd) => self.audio_tx.send(cmd),
                        UiPayload::DungeonCallout {
                            display_name,
                            modifiers,
                            grade,
                            origin,
                        } => {
                            self.refresh_auto_callout(
                                &display_name,
                                &modifiers,
                                &grade,
                                origin.as_ref(),
                            );
                        }
                        payload => {
                            if let UiPayload::Chat(message) = &payload {
                                if let Err(error) = self.chat_log.append(message) {
                                    tracing::warn!("[CHAT] failed to write log: {error}");
                                    self.health
                                        .record_persistence_failure(format!("chat log: {error}"));
                                }
                            }
                            if let UiPayload::AccessTokenCaptured(token) = &payload {
                                self.fetch_exalts_once(token);
                            }
                            if let UiPayload::Broadcast(event) = &payload {
                                let local_leader = self.processor.party_view().is_local_leader;
                                for alert in party_alerts(&self.watchlist, event, local_leader) {
                                    self.processor.record_live_event(alert);
                                }
                            }
                            had_ui_updates = true;
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
                if had_ui_updates || view_changed {
                    self.wake.request(&self.ctx);
                }
            } else if missions_changed || identity_changed {
                #[cfg(feature = "latency-diagnostics")]
                let snapshot_started = Instant::now();
                self.publish_snapshot();
                #[cfg(feature = "latency-diagnostics")]
                {
                    segments.snapshot = snapshot_started.elapsed();
                }
                self.wake.request(&self.ctx);
            }

            if queue_remaining == 0
                && self
                    .next_maintenance
                    .is_some_and(|due| Instant::now() >= due)
            {
                maintenance_log.begin_batch("history-maintenance");
                let delay = match self.processor.historical_maintenance_batch() {
                    Ok((changed, pending)) => {
                        maintenance_log.finish(&Ok(()));
                        self.maintenance_history_dirty |= changed;
                        if self.maintenance_history_dirty
                            && (!pending || Instant::now() >= self.next_maintenance_refresh)
                        {
                            self.processor.publish_repaired_history();
                            self.maintenance_history_dirty = false;
                            self.next_maintenance_refresh = Instant::now() + Duration::from_secs(5);
                        }
                        if pending {
                            Duration::from_millis(50)
                        } else {
                            Duration::from_secs(1)
                        }
                    }
                    Err(error) => {
                        maintenance_log.failed(&error);
                        Duration::from_secs(5)
                    }
                };
                self.next_maintenance = Some(Instant::now() + delay);
                if self.publish_snapshot() {
                    self.wake.request(&self.ctx);
                }
            }

            // 4. Interim periodic persistence.
            #[cfg(feature = "latency-diagnostics")]
            let save_started = Instant::now();
            self.maybe_periodic_save();
            self.maybe_log_health();
            for error in self.processor.drain_loot_persistence_errors() {
                self.health
                    .record_persistence_failure(format!("loot insert: {error}"));
            }
            self.health
                .set_live_window_evictions(self.processor.live_event_evictions());
            let (audio_stale, audio_full) = crate::audio::drop_counts();
            self.health.set_audio_drops(audio_stale, audio_full);
            self.expire_auto_callout();
            if self.processor.take_realm_close() {
                self.release_auto_callout();
            }
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
        self.release_auto_callout();
        self.processor.final_drain();
        for error in self.processor.drain_loot_persistence_errors() {
            self.health
                .record_persistence_failure(format!("loot insert: {error}"));
        }
        let persistence = self.processor.save_account_on_exit();
        let db_close = self.processor.close_databases();
        let report = ShutdownReport::from_results(persistence, db_close);
        match &report {
            ShutdownReport::PersistenceFailed(error) => {
                self.health
                    .record_persistence_failure(format!("account save: {error}"));
            }
            ShutdownReport::DatabaseCloseFailed(error) => {
                self.health
                    .record_persistence_failure(format!("database close: {error}"));
            }
            ShutdownReport::Success | ShutdownReport::TimedOut => {}
        }
        self.health.set_capture_state(CaptureState::Stopped);
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
            ControlMsg::StartHistoricalMaintenance => {
                self.next_maintenance.get_or_insert_with(Instant::now);
            }
            ControlMsg::SetWatchlist(watchlist) => {
                self.watchlist = watchlist;
            }
            ControlMsg::SetLiveDemand(demand) => {
                self.wake.set_demand(demand);
            }
            ControlMsg::StartCapture(receiver) => {
                self.processor.on_capture_start();
                #[cfg(feature = "latency-diagnostics")]
                if let Some(previous) = self.receiver.take() {
                    self.capture_drops.finish_source(previous.dropped());
                }
                self.receiver = Some(receiver);
                self.health.set_capture_state(CaptureState::Active);
                // Reset publishes so the UI re-syncs after a capture restart.
                self.publish_snapshot();
                self.wake.request(&self.ctx);
            }
            ControlMsg::StopCapture => {
                self.processor.on_capture_stop();
                #[cfg(feature = "latency-diagnostics")]
                if let Some(receiver) = self.receiver.take() {
                    self.capture_drops.finish_source(receiver.dropped());
                }
                #[cfg(not(feature = "latency-diagnostics"))]
                {
                    self.receiver = None;
                }
                self.release_auto_callout();
                self.health.set_capture_state(CaptureState::Stopped);
            }
            other => {
                self.processor.apply_control(other);
                let view_changed = self.publish_snapshot();
                if view_changed {
                    self.wake.request(&self.ctx);
                }
            }
        }
        false
    }

    /// Release any automatic callout this worker put on the clipboard.
    fn release_auto_callout(&mut self) {
        if let Some(auto) = self.auto_callout.take() {
            crate::clipboard::clear_if_ours(&auto.text);
        }
    }

    /// Clear the automatic callout once its join window elapses.
    fn expire_auto_callout(&mut self) {
        if self
            .auto_callout
            .as_ref()
            .is_some_and(|auto| Instant::now() >= auto.expires_at)
        {
            self.release_auto_callout();
        }
    }

    /// Copy the new dungeon's callout worker-side when auto-clipboard is enabled.
    fn refresh_auto_callout(
        &mut self,
        display_name: &str,
        modifiers: &[String],
        grade: &Option<String>,
        origin: Option<&super::contract::DungeonOrigin>,
    ) {
        self.release_auto_callout();
        let Some(origin) = origin else {
            return;
        };
        let settings = match self.settings.read() {
            Ok(settings) => settings.clone(),
            Err(_) => return,
        };
        let learned = self.processor.learned_event_mods(
            &crate::panels::live_feed::format_dungeon_name(display_name),
            settings.live_feed.learn_event_mods,
            settings.live_feed.learn_event_mods_runs.clamp(
                *realmhound_core::settings::LEARN_EVENT_MOD_RUNS_RANGE.start(),
                *realmhound_core::settings::LEARN_EVENT_MOD_RUNS_RANGE.end(),
            ) as usize,
        );
        let Some((text, expires_at)) =
            build_auto_callout(&settings, display_name, modifiers, grade, &learned, origin)
        else {
            return;
        };
        if crate::clipboard::set_text(&text) {
            self.auto_callout = Some(AutoCallout { text, expires_at });
        }
    }

    /// Fetch exalt stats once per worker session, off the UI drain path.
    fn fetch_exalts_once(&mut self, token: &str) {
        if self.exalts_fetched {
            return;
        }
        self.exalts_fetched = true;
        let token = token.to_string();
        std::thread::spawn(move || {
            tracing::debug!("[API] Fetching exalt stats...");
            let client = realmhound_core::api::RotmgApiClient::new(token);
            match client.get_power_up_stats() {
                Ok(xml) => tracing::info!("[EXALTS] Loaded exalt stats ({} bytes)", xml.len()),
                Err(error) => tracing::warn!("[EXALTS] Failed to load: {}", error),
            }
        });
    }

    /// Periodic account save, mirroring the App's prior 60s/dirty policy.
    fn maybe_periodic_save(&mut self) {
        if self.last_save.elapsed() >= SAVE_INTERVAL {
            match self.processor.save_account_if_dirty() {
                Ok(true) => self.last_save = Instant::now(),
                Ok(false) => {}
                Err(error) => {
                    self.health
                        .record_persistence_failure(format!("periodic account save: {error}"));
                    self.last_save = Instant::now();
                }
            }
        }
    }

    /// Throttled health summary that does not depend on the UI painting.
    ///
    /// Frame measurements stay in the UI-side monitor; this line covers the
    /// worker-owned numbers so a stalled or minimized window is still visible.
    fn maybe_log_health(&mut self) {
        if !super::health::pipeline_logs_enabled()
            || self.last_health_log.elapsed() < HEALTH_LOG_INTERVAL
        {
            return;
        }
        self.last_health_log = Instant::now();
        let counters = self.health.snapshot(0, 0);
        tracing::info!(
            "[HEALTH][WORKER] ui_backlog={} ui_drops={} capture={} persistence_failures={} \
             live_window_events={} live_window_evictions={} audio_stale_drops={} \
             audio_full_drops={}",
            self.ui_tx.len(),
            self.ui_tx.dropped(),
            counters.capture.label(),
            counters.persistence_failures,
            self.processor.live_event_window().events.len(),
            counters.live_window_evictions,
            counters.audio_stale_drops,
            counters.audio_full_drops
        );
    }

    /// Publish the latest model state into the shared snapshot, cloning heavy
    /// fields only when their generation changed. Returns whether the scalar
    /// view state changed (so the caller can request a repaint).
    fn publish_snapshot(&mut self) -> bool {
        let started = Instant::now();
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
        let token_gen = view.token_gen;
        let token_changed = token_gen != self.pub_token_gen;
        let party_rev = view.party_rev;
        let party_changed = party_rev != self.pub_party_rev;
        let encounter_rev = view.encounter_rev;
        let encounters_changed = encounter_rev != self.pub_encounter_rev;
        let quest_rev = view.quest_rev;
        let quests_changed = quest_rev != self.pub_quest_rev;
        let live_event_rev = view.live_event_rev;
        let live_events_changed = live_event_rev != self.pub_live_event_rev;
        let feed_rev = view.feed_rev;
        let feed_changed = feed_rev != self.pub_feed_rev;
        let view_changed = self.pub_view.as_ref() != Some(&view);

        let lock_started = Instant::now();
        let previous = match self.snapshot.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => return false,
        };
        self.health
            .record_snapshot_lock_wait(lock_started.elapsed());

        // Heavy clones happen outside the lock; unchanged domains reuse their Arc.
        let account_data = account_changed.then(|| Arc::new(self.processor.account_data().clone()));
        let live_vault_data = vault_changed.then(|| Arc::new(self.processor.live_vault_data()));
        let mission_view = mission_changed.then(|| Arc::new(self.processor.mission_view()));
        let token = token_changed
            .then(|| self.processor.token_state().map(Arc::new))
            .flatten();
        let party = party_changed.then(|| Arc::new(self.processor.party_view()));
        let encounters = encounters_changed.then(|| Arc::new(self.processor.encounter_set()));
        let quests = quests_changed
            .then(|| self.processor.quest_view().map(Arc::new))
            .flatten();
        let live_events = live_events_changed.then(|| Arc::new(self.processor.live_event_window()));
        let feed = feed_changed.then(|| Arc::new(self.processor.feed_context()));
        let next = previous.next(
            view.clone(),
            live_char_id,
            main_connection,
            account_data,
            live_vault_data,
            mission_view,
            token,
            party,
            encounters,
            quests,
            live_events,
            feed,
        );
        let lock_started = Instant::now();
        let replaced = match self.snapshot.lock() {
            Ok(mut slot) => std::mem::replace(&mut *slot, Arc::new(next)),
            Err(_) => return false,
        };
        self.health
            .record_snapshot_lock_wait(lock_started.elapsed());
        self.health.record_snapshot_publish(started.elapsed());
        drop(replaced);

        self.pub_char_gen = char_gen;
        self.pub_account_gen = account_gen;
        self.pub_vault_gen = vault_gen;
        self.pub_mission_gen = mission_gen;
        self.pub_token_gen = token_gen;
        self.pub_party_rev = party_rev;
        self.pub_encounter_rev = encounter_rev;
        self.pub_quest_rev = quest_rev;
        self.pub_live_event_rev = live_event_rev;
        self.pub_feed_rev = feed_rev;
        self.pub_view = Some(view);
        view_changed
    }
}

/// Watchlist alerts carried by a party event.
fn party_alerts(
    watchlist: &Watchlist,
    event: &GameEvent,
    local_leader: bool,
) -> Vec<LiveEventView> {
    let mut alerts = Vec::new();
    let mut kick_if_banned = |name: &str| {
        if watchlist.check_player(name) == PlayerStatus::Banned {
            alerts.push(LiveEventView::KickAlert {
                name: name.to_string(),
            });
        }
    };
    match event {
        GameEvent::PartyListReceived(info) => {
            for player in &info.party_players {
                kick_if_banned(&player.name);
            }
        }
        GameEvent::PartyMemberJoined(member) => kick_if_banned(&member.name),
        GameEvent::PartyJoinRequestResponse(response) if local_leader && response.state == 1 => {
            if watchlist.check_player(&response.player_name) == PlayerStatus::Banned {
                alerts.push(LiveEventView::JoinRequestAlert {
                    name: response.player_name.clone(),
                });
            }
        }
        _ => {}
    }
    alerts
}

/// Build the automatic dungeon callout, if one should be copied.
fn build_auto_callout(
    settings: &Settings,
    display_name: &str,
    modifiers: &[String],
    grade: &Option<String>,
    learned: &[String],
    origin: &super::contract::DungeonOrigin,
) -> Option<(String, Instant)> {
    if !settings.live_feed.auto_clipboard_dungeon_calls {
        return None;
    }
    let estimated = origin.portal_anchor.is_none();
    let score = origin.realm_score_percent;
    if !origin.realm_closed && score.is_some_and(|percent| percent >= 100) {
        return None;
    }
    let origin_status = match crate::panels::dungeon_callout::realm_status_suffix(
        settings.live_feed.realm_status,
        settings.live_feed.realm_status_threshold,
        score,
    ) {
        Some(status) => Some(status),
        None if origin.entered_from_nexus => {
            settings.live_feed.nexus_marker.text().map(str::to_string)
        }
        None => None,
    };
    let params = crate::panels::dungeon_callout::DungeonCalloutParams {
        name_style: settings.live_feed.dungeon_name_style,
        name_overrides: &settings.live_feed.dungeon_name_overrides,
        loot_label: settings.live_feed.loot_label,
        dust_label: settings.live_feed.dust_label,
        xp_label: settings.live_feed.xp_label,
        loot_threshold: settings.live_feed.loot_threshold,
        dust_threshold: settings.live_feed.dust_threshold,
        xp_threshold: settings.live_feed.xp_threshold,
        call_event_mods: settings.live_feed.call_event_mods,
        percent: settings.live_feed.callout_percent,
        reward_mods: &settings.live_feed.reward_mods,
        learned_event_mods: learned,
        origin_status: origin_status.as_deref(),
    };
    // Anchor the countdown to the portal the player used, not to the moment
    // this batch happened to be processed.
    // A known portal-spawn anchor means the countdown is NOT estimated.
    let anchor = origin
        .portal_anchor
        .or(origin.entered_at)
        .unwrap_or_else(Instant::now);
    let entry = crate::panels::live_feed::DungeonEntry::new(
        display_name.to_string(),
        None,
        modifiers,
        grade.clone(),
        &params,
        origin.server_name.clone(),
        origin.realm_name.clone(),
        score,
        anchor,
        estimated,
    );
    let body = entry.callout.as_deref()?;
    if entry.remaining(Instant::now()).is_zero() {
        return None;
    }
    let text = crate::panels::dungeon_callout::assemble_callout(
        body,
        entry.server_name.as_deref(),
        entry.realm_name.as_deref(),
        settings.live_feed.dungeon_join_position,
        settings.live_feed.call_for_party,
        settings.live_feed.include_server_name,
        settings.live_feed.include_realm_name,
    );
    Some((text, entry.expires_at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn party_alerts_follow_the_worker_watchlist() {
        use realmhound_core::protocol::{PartyMemberAddedPacket, PartyRequestResponsePacket};

        let mut watchlist = Watchlist::new();
        watchlist.replace_entries("Bad");
        assert_eq!(watchlist.check_player("Bad"), PlayerStatus::Banned);

        let joined = party_alerts(
            &watchlist,
            &GameEvent::PartyMemberJoined(PartyMemberAddedPacket {
                player_id: 1,
                name: "Bad".to_string(),
                class_id: 0,
                skin_id: 0,
            }),
            false,
        );
        assert!(matches!(
            joined.as_slice(),
            [LiveEventView::KickAlert { name }] if name == "Bad"
        ));

        let requested = party_alerts(
            &watchlist,
            &GameEvent::PartyJoinRequestResponse(PartyRequestResponsePacket {
                player_name: "Bad".to_string(),
                class_id: 0,
                skin_id: 0,
                state: 1,
            }),
            true,
        );
        assert!(matches!(
            requested.as_slice(),
            [LiveEventView::JoinRequestAlert { name }] if name == "Bad"
        ));
    }

    #[test]
    fn auto_callout_builds_joinable_callout() {
        let _assets = crate::test_support::modifier_assets();
        let mut settings = Settings::default();
        settings.live_feed.auto_clipboard_dungeon_calls = true;
        let (text, _) = build_auto_callout(
            &settings,
            "Snake Pit",
            &[],
            &None,
            &[],
            &crate::processing::contract::DungeonOrigin {
                server_name: Some("EUWest".to_string()),
                realm_name: Some("Mesa".to_string()),
                ..Default::default()
            },
        )
        .expect("joinable dungeon produces a callout");
        assert!(text.contains("snake"), "unexpected callout: {text}");
    }

    #[test]
    fn auto_callout_requires_setting_and_callable_dungeon() {
        let mut settings = Settings::default();
        assert!(
            build_auto_callout(&settings, "Snake Pit", &[], &None, &[], &Default::default())
                .is_none()
        );

        settings.live_feed.auto_clipboard_dungeon_calls = true;
        assert!(
            build_auto_callout(&settings, "The Void", &[], &None, &[], &Default::default())
                .is_none()
        );
    }

    #[test]
    fn auto_callout_uses_resolved_origin_and_original_deadline() {
        use crate::processing::contract::DungeonOrigin;
        let _assets = crate::test_support::modifier_assets();
        let mut settings = Settings::default();
        settings.live_feed.auto_clipboard_dungeon_calls = true;
        settings.live_feed.realm_status = realmhound_core::settings::RealmStatusMode::Score;
        settings.live_feed.realm_status_threshold = 0;
        let entered_at = Instant::now();
        let mut origin = DungeonOrigin {
            entered_at: Some(entered_at),
            portal_anchor: Some(entered_at - Duration::from_secs(5)),
            realm_score_percent: Some(74),
            ..Default::default()
        };
        let (text, deadline) =
            build_auto_callout(&settings, "Snake Pit", &[], &None, &[], &origin).unwrap();
        assert!(text.contains("in 74% realm"), "{text}");
        assert_eq!(
            deadline,
            origin.portal_anchor.unwrap() + Duration::from_secs(30)
        );
        origin.portal_anchor = None;
        origin.realm_score_percent = None;
        origin.entered_from_nexus = true;
        let (text, deadline) =
            build_auto_callout(&settings, "Snake Pit", &[], &None, &[], &origin).unwrap();
        assert!(!text.contains("realm"), "{text}");
        assert_eq!(deadline, entered_at + Duration::from_secs(25));
        origin.entered_at = Some(entered_at - Duration::from_secs(60));
        assert!(build_auto_callout(&settings, "Snake Pit", &[], &None, &[], &origin).is_none());
    }

    #[test]
    fn wake_throttle_coalesces_and_keeps_a_trailing_repaint() {
        let t0 = Instant::now();
        let mut wake = WakeThrottle::default();
        assert_eq!(wake.next_delay(t0), None);
        assert_eq!(
            wake.next_delay(t0 + Duration::from_millis(10)),
            Some(Duration::from_millis(90))
        );
        assert_eq!(
            wake.next_delay(t0 + Duration::from_millis(99)),
            Some(Duration::from_millis(1))
        );
        assert_eq!(wake.next_delay(t0 + Duration::from_millis(100)), None);
    }

    #[test]
    fn restored_demand_clears_the_throttle_window() {
        let t0 = Instant::now();
        let mut wake = WakeThrottle::default();
        assert_eq!(wake.next_delay(t0), None);
        wake.set_demand(false);
        wake.set_demand(true);
        assert_eq!(wake.next_delay(t0 + Duration::from_millis(10)), None);
    }

    #[cfg(feature = "latency-diagnostics")]
    #[test]
    fn packet_latency_separates_pre_queue_and_queue_time() {
        let drained_at = Instant::now();
        let packet = realmhound_core::capture::RawPacket {
            timestamp: chrono::Utc::now() - chrono::Duration::milliseconds(700),
            packet_format: realmhound_core::capture::PacketFormat::Ethernet,
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
            packet_format: realmhound_core::capture::PacketFormat::Ethernet,
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
            packet_format: realmhound_core::capture::PacketFormat::Ethernet,
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
            account.clone(),
        )
        .expect("worker thread spawns");

        // Once activated, a clean shutdown flushes the (empty) in-memory
        // snapshot, overwriting the marker -- proving activation gates persistence.
        worker.activate();
        worker.shutdown(std::time::Duration::from_secs(5));
        assert_eq!(stored_generation(&account), 0);
    }

    #[test]
    fn historical_maintenance_waits_for_the_first_completed_ui_frame() {
        let temp = tempfile::tempdir().unwrap();
        let root = StorageRoot::from_path(temp.path()).unwrap();
        let account = selected_context(&root);
        let processor = processor(&account);
        let reader = realmhound_core::combat::CombatDatabase::open_reader_at(
            account.persistence().combat_database(),
        )
        .unwrap();
        let before = reader.data_version().unwrap();
        let mut worker = spawn(
            processor,
            egui::Context::default(),
            AudioSender::disconnected(),
            account,
        )
        .unwrap();
        worker.activate();
        std::thread::sleep(Duration::from_millis(250));
        let before_ready = reader.data_version().unwrap();
        worker.send_control(ControlMsg::StartHistoricalMaintenance);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut changed = false;
        while Instant::now() < deadline {
            if reader.data_version().unwrap() != before {
                changed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        worker.shutdown(Duration::from_secs(5));
        assert_eq!(
            before_ready, before,
            "activation alone must not start repairs"
        );
        assert!(
            changed,
            "the UI-ready signal automatically starts repairs without capture"
        );
    }
}

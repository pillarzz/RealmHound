//! Dedicated audio thread.
//!
//! rodio's `OutputStream` is `!Send`, so audio playback must happen on a
//! dedicated thread. The [`SoundEngine`] recreates the stream on each play so
//! it always targets the current system default output device.
//! The worker (and the UI for the volume slider / test button) send commands
//! via a bounded [`AudioCommand`] channel.
//!
//! Duplicate `Play(same type)` within a short window are coalesced so that a
//! post-backlog burst (e.g. catching up after the window was minimized) doesn't
//! fire a flurry of identical sounds.

use std::collections::HashMap;
#[cfg(feature = "latency-diagnostics")]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
#[cfg(feature = "latency-diagnostics")]
use std::sync::Mutex;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

use realmhound_core::Settings;

use crate::processing::AudioCommand;
#[cfg(feature = "latency-diagnostics")]
use crate::sound::{record_audio_channel_full, record_audio_command};
use crate::sound::{resolve_custom_sound, resolve_custom_sound_key, SoundEngine, SoundType};

/// Bound on the audio command channel. Small: audio is latency-tolerant and we
/// coalesce duplicates anyway, so a backlog should never accumulate.
const AUDIO_CHANNEL_BOUND: usize = 64;

/// Window within which a duplicate `Play(same resolved sound)` is suppressed.
const COALESCE_WINDOW: Duration = Duration::from_millis(150);

/// Age past which a queued `Play`/`PlayForBag` is dropped instead of played.
/// Safety net for suspend/sleep: pre-sleep commands sit in the channel while the
/// thread is frozen and would otherwise burst on resume. Uses wall-clock
/// `SystemTime` since only it includes time spent suspended.
const STALE_THRESHOLD: Duration = Duration::from_secs(5);

struct TimedCommand {
    cmd: AudioCommand,
    sent: SystemTime,
    #[cfg(feature = "latency-diagnostics")]
    dispatched_at: Instant,
}

impl TimedCommand {
    fn command_kind(&self) -> &'static str {
        match &self.cmd {
            AudioCommand::Play(_) => "play",
            AudioCommand::PlayForBag(_) => "play_for_bag",
            AudioCommand::PlayEvent { .. } => "play_event",
            AudioCommand::SetVolume(_) => "set_volume",
        }
    }
}

#[cfg(feature = "latency-diagnostics")]
static FULL_DROPS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "latency-diagnostics")]
static LAST_FULL_WARN: Mutex<Option<Instant>> = Mutex::new(None);

#[cfg(feature = "latency-diagnostics")]
#[derive(Default)]
struct StaleDropDiagnostics {
    total: u64,
    since_warn: u64,
    peak_age: Duration,
    last_warn: Option<Instant>,
}

#[cfg(feature = "latency-diagnostics")]
struct StaleDropWarning {
    total: u64,
    since_warn: u64,
    peak_age: Duration,
}

#[cfg(feature = "latency-diagnostics")]
impl StaleDropDiagnostics {
    fn record(
        &mut self,
        now: Instant,
        age: Duration,
        throttle: Duration,
    ) -> Option<StaleDropWarning> {
        self.total = self.total.saturating_add(1);
        self.since_warn = self.since_warn.saturating_add(1);
        self.peak_age = self.peak_age.max(age);
        if self
            .last_warn
            .is_some_and(|last| now.duration_since(last) < throttle)
        {
            return None;
        }
        self.last_warn = Some(now);
        let warning = StaleDropWarning {
            total: self.total,
            since_warn: self.since_warn,
            peak_age: self.peak_age,
        };
        self.since_warn = 0;
        self.peak_age = Duration::ZERO;
        Some(warning)
    }
}

#[cfg(feature = "latency-diagnostics")]
#[derive(Default)]
struct QueueLatencyDiagnostics {
    total: u64,
    since_warn: u64,
    peak_age: Duration,
    last_warn: Option<Instant>,
}

#[cfg(feature = "latency-diagnostics")]
struct QueueLatencyWarning {
    total: u64,
    since_warn: u64,
    peak_age: Duration,
}

#[cfg(feature = "latency-diagnostics")]
impl QueueLatencyDiagnostics {
    fn record(
        &mut self,
        now: Instant,
        age: Duration,
        throttle: Duration,
    ) -> Option<QueueLatencyWarning> {
        self.total = self.total.saturating_add(1);
        self.since_warn = self.since_warn.saturating_add(1);
        self.peak_age = self.peak_age.max(age);
        if self
            .last_warn
            .is_some_and(|last| now.duration_since(last) < throttle)
        {
            return None;
        }
        self.last_warn = Some(now);
        let warning = QueueLatencyWarning {
            total: self.total,
            since_warn: self.since_warn,
            peak_age: self.peak_age,
        };
        self.since_warn = 0;
        self.peak_age = Duration::ZERO;
        Some(warning)
    }
}

/// Handle to the audio thread: a cloneable sender plus the join handle.
pub struct AudioHandle {
    sender: SyncSender<TimedCommand>,
    /// Set to `false` by the thread if the `SoundEngine` failed to initialise.
    available: Arc<std::sync::atomic::AtomicBool>,
    /// Kept so the thread stays owned; detached on drop (the audio thread is a
    /// daemon with no persistent state to flush, so we never block-join it —
    /// blocking would hang if any `AudioSender` clone outlives this handle).
    _join: std::thread::JoinHandle<()>,
}

impl AudioHandle {
    /// A cloneable sender for audio commands.
    pub fn sender(&self) -> AudioSender {
        AudioSender {
            tx: self.sender.clone(),
        }
    }

    /// Whether the audio output initialised successfully.
    pub fn is_available(&self) -> bool {
        self.available.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// A cloneable, non-blocking audio command sender.
#[derive(Clone)]
pub struct AudioSender {
    tx: SyncSender<TimedCommand>,
}

impl AudioSender {
    /// Send a command without blocking. Drops the command if the channel is full
    /// (audio is non-critical) or the thread has exited.
    pub fn send(&self, cmd: AudioCommand) {
        let timed = TimedCommand {
            cmd,
            sent: SystemTime::now(),
            #[cfg(feature = "latency-diagnostics")]
            dispatched_at: Instant::now(),
        };
        match self.tx.try_send(timed) {
            Ok(()) => {}
            Err(TrySendError::Full(_timed)) => {
                #[cfg(feature = "latency-diagnostics")]
                {
                    record_audio_channel_full();
                    let now = Instant::now();
                    let total = FULL_DROPS.fetch_add(1, Ordering::Relaxed) + 1;
                    let mut last = LAST_FULL_WARN.lock().unwrap_or_else(|e| e.into_inner());
                    if last.is_none_or(|at| now.duration_since(at) >= Duration::from_secs(2)) {
                        *last = Some(now);
                        tracing::warn!(
                            "[LATENCY][AUDIO] stage=channel_full command_kind={} \
                             dropped_total={}",
                            _timed.command_kind(),
                            total
                        );
                    }
                }
                #[cfg(not(feature = "latency-diagnostics"))]
                tracing::trace!("[AUDIO] command channel full, dropping sound");
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// A sender with no listening audio thread; every send is silently dropped.
    /// For tests that need an `AudioSender` without opening an audio device.
    #[cfg(test)]
    pub fn disconnected() -> Self {
        let (tx, _rx) = sync_channel::<TimedCommand>(1);
        Self { tx }
    }
}

/// Spawn the audio thread, returning a handle. The thread holds a clone of the
/// shared settings to resolve `PlayForBag` and to initialise the volume.
pub fn spawn(settings: Arc<RwLock<Settings>>) -> AudioHandle {
    let (tx, rx) = sync_channel::<TimedCommand>(AUDIO_CHANNEL_BOUND);
    let available = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let available_thread = available.clone();

    let join = std::thread::Builder::new()
        .name("realmhound-audio".into())
        .spawn(move || run(rx, settings, available_thread))
        .expect("failed to spawn audio thread");

    AudioHandle {
        sender: tx,
        available,
        _join: join,
    }
}

fn run(
    rx: std::sync::mpsc::Receiver<TimedCommand>,
    settings: Arc<RwLock<Settings>>,
    _available: Arc<std::sync::atomic::AtomicBool>,
) {
    let mut engine = SoundEngine::new();

    // Initialise volume from current settings.
    if let Ok(s) = settings.read() {
        engine.set_volume(s.sound.volume);
    }

    // Last time each resolved sound type was played, for coalescing.
    let mut last_played: HashMap<SoundType, Instant> = HashMap::new();
    // Last time each realm-event custom key played, for coalescing.
    let mut last_event: HashMap<String, Instant> = HashMap::new();
    #[cfg(feature = "latency-diagnostics")]
    let mut stale_drops = StaleDropDiagnostics::default();
    #[cfg(feature = "latency-diagnostics")]
    let mut queue_latency = QueueLatencyDiagnostics::default();

    for timed in rx.iter() {
        // Skip notifications that went stale while the thread was suspended
        // (sleep); `elapsed` errs only on backwards clock, treat as fresh.
        let age = timed.sent.elapsed().unwrap_or(Duration::ZERO);
        let is_stale = age > STALE_THRESHOLD && !matches!(&timed.cmd, AudioCommand::SetVolume(_));
        #[cfg(feature = "latency-diagnostics")]
        let command_kind = timed.command_kind();
        #[cfg(feature = "latency-diagnostics")]
        let queue_age = timed.dispatched_at.elapsed();
        #[cfg(feature = "latency-diagnostics")]
        record_audio_command(command_kind, queue_age, is_stale);
        if is_stale {
            #[cfg(feature = "latency-diagnostics")]
            if let Some(warning) = stale_drops.record(Instant::now(), age, Duration::from_secs(2)) {
                tracing::warn!(
                    "[LATENCY][AUDIO] stage=stale_drop command_kind={} dropped_since_warn={} \
                     dropped_total={} peak_queued_ms={}",
                    command_kind,
                    warning.since_warn,
                    warning.total,
                    warning.peak_age.as_millis()
                );
            }
            #[cfg(not(feature = "latency-diagnostics"))]
            tracing::debug!(
                "[AUDIO] dropping stale command kind={} queued_ms={}",
                timed.command_kind(),
                age.as_millis()
            );
            continue;
        }
        #[cfg(feature = "latency-diagnostics")]
        {
            if queue_age >= Duration::from_millis(250) {
                if let Some(warning) =
                    queue_latency.record(Instant::now(), queue_age, Duration::from_secs(2))
                {
                    tracing::warn!(
                        "[LATENCY][AUDIO] stage=queue command_kind={} delayed_since_warn={} \
                         delayed_total={} peak_queued_ms={}",
                        command_kind,
                        warning.since_warn,
                        warning.total,
                        warning.peak_age.as_millis()
                    );
                }
            }
        }

        match timed.cmd {
            AudioCommand::SetVolume(v) => {
                engine.set_volume(v);
            }
            AudioCommand::Play(sound) => {
                let (per_vol, custom_path) = settings
                    .read()
                    .map(|s| {
                        (
                            s.sound.sound_volume(sound.settings_key()),
                            resolve_custom_sound(&s.sound, sound),
                        )
                    })
                    .unwrap_or((1.0, None));
                let volume = engine.volume() * per_vol;
                play_coalesced(
                    &engine,
                    &mut last_played,
                    sound,
                    custom_path,
                    volume,
                    #[cfg(feature = "latency-diagnostics")]
                    timed.dispatched_at,
                );
            }
            AudioCommand::PlayForBag(bag) => {
                // Resolve here (needs current sound settings) then coalesce on
                // the resolved sound type, same as a direct `Play`.
                if let Some(sound) = SoundType::from_bag_type(bag) {
                    let (enabled, per_vol, custom_path) = settings
                        .read()
                        .map(|s| {
                            let en = sound.is_enabled(&s.sound);
                            let vol = s.sound.sound_volume(sound.settings_key());
                            let cp = resolve_custom_sound(&s.sound, sound);
                            (en, vol, cp)
                        })
                        .unwrap_or((false, 1.0, None));
                    if enabled {
                        let volume = engine.volume() * per_vol;
                        play_coalesced(
                            &engine,
                            &mut last_played,
                            sound,
                            custom_path,
                            volume,
                            #[cfg(feature = "latency-diagnostics")]
                            timed.dispatched_at,
                        );
                    }
                }
            }
            AudioCommand::PlayEvent {
                sound,
                custom_key,
                volume,
            } => {
                // Resolve the per-boss custom override and current master, then
                // play at the per-event volume scaled by master. Coalesced on
                // the custom key so repeated identical calls don't stack.
                let (master, custom_path) = settings
                    .read()
                    .map(|s| {
                        (
                            s.sound.volume,
                            resolve_custom_sound_key(&s.sound, &custom_key),
                        )
                    })
                    .unwrap_or((1.0, None));
                let now = Instant::now();
                if let Some(prev) = last_event.get(&custom_key) {
                    if now.duration_since(*prev) < COALESCE_WINDOW {
                        tracing::trace!("[AUDIO] coalescing duplicate event {}", custom_key);
                        continue;
                    }
                }
                last_event.insert(custom_key, now);
                engine.play_at(
                    sound,
                    custom_path,
                    volume * master,
                    #[cfg(feature = "latency-diagnostics")]
                    timed.dispatched_at,
                );
            }
        }
    }

    tracing::info!("[AUDIO] audio thread stopped");
}

/// Play `sound` unless an identical sound played within [`COALESCE_WINDOW`].
fn play_coalesced(
    engine: &SoundEngine,
    last_played: &mut HashMap<SoundType, Instant>,
    sound: SoundType,
    custom_path: Option<std::path::PathBuf>,
    volume: f32,
    #[cfg(feature = "latency-diagnostics")] dispatched_at: Instant,
) {
    let now = Instant::now();
    if let Some(prev) = last_played.get(&sound) {
        if now.duration_since(*prev) < COALESCE_WINDOW {
            tracing::trace!("[AUDIO] coalescing duplicate {:?}", sound);
            return;
        }
    }
    last_played.insert(sound, now);
    engine.play(
        sound,
        custom_path,
        volume,
        #[cfg(feature = "latency-diagnostics")]
        dispatched_at,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_kind_is_safe_and_stable() {
        let timed = TimedCommand {
            cmd: AudioCommand::PlayEvent {
                sound: crate::sound::EventSound::Veteran,
                custom_key: "private-custom-key".to_string(),
                volume: 1.0,
            },
            sent: SystemTime::now(),
            #[cfg(feature = "latency-diagnostics")]
            dispatched_at: Instant::now(),
        };
        assert_eq!(timed.command_kind(), "play_event");
    }

    #[cfg(feature = "latency-diagnostics")]
    #[test]
    fn stale_drop_diagnostics_aggregate_within_throttle() {
        let now = Instant::now();
        let mut diagnostics = StaleDropDiagnostics::default();
        let first = diagnostics
            .record(now, Duration::from_secs(6), Duration::from_secs(2))
            .unwrap();
        assert_eq!(first.total, 1);
        assert!(diagnostics
            .record(
                now + Duration::from_millis(100),
                Duration::from_secs(9),
                Duration::from_secs(2)
            )
            .is_none());
        let summary = diagnostics
            .record(
                now + Duration::from_secs(2),
                Duration::from_secs(7),
                Duration::from_secs(2),
            )
            .unwrap();
        assert_eq!(summary.since_warn, 2);
        assert_eq!(summary.total, 3);
        assert_eq!(summary.peak_age, Duration::from_secs(9));
        assert_eq!(diagnostics.peak_age, Duration::ZERO);
    }

    #[cfg(feature = "latency-diagnostics")]
    #[test]
    fn queue_latency_diagnostics_aggregate_within_throttle() {
        let now = Instant::now();
        let mut diagnostics = QueueLatencyDiagnostics::default();
        let first = diagnostics
            .record(now, Duration::from_millis(300), Duration::from_secs(2))
            .unwrap();
        assert_eq!(first.total, 1);
        assert!(diagnostics
            .record(
                now + Duration::from_millis(100),
                Duration::from_millis(900),
                Duration::from_secs(2)
            )
            .is_none());
        let summary = diagnostics
            .record(
                now + Duration::from_secs(2),
                Duration::from_millis(500),
                Duration::from_secs(2),
            )
            .unwrap();
        assert_eq!(summary.since_warn, 2);
        assert_eq!(summary.total, 3);
        assert_eq!(summary.peak_age, Duration::from_millis(900));
    }
}

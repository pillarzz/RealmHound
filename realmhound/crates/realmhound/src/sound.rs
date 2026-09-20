//! Sound notification engine.
//!
//! Provides audio playback for loot drop notifications and other events.
//! Public default sounds are from RealmShark under the MIT license.

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;

use rodio::{Decoder, OutputStream, Sink};

use realmhound_core::loot::LootBagType;
use realmhound_core::settings::SoundSettings;

// Embed sound files at compile time
const WHITEBAG_WAV: &[u8] = include_bytes!("../assets/sounds/whitebag.wav");
const ORANGEBAG_WAV: &[u8] = include_bytes!("../assets/sounds/orangebag.wav");
const REDBAG_WAV: &[u8] = include_bytes!("../assets/sounds/redbag.wav");
const BLUEBAG_WAV: &[u8] = include_bytes!("../assets/sounds/bluebag.wav");
const GOLDBAG_WAV: &[u8] = include_bytes!("../assets/sounds/goldbag.wav");
const EGGBAG_WAV: &[u8] = include_bytes!("../assets/sounds/eggbag.wav");
const KEYPOP_WAV: &[u8] = include_bytes!("../assets/sounds/keypop.wav");
const PARTY_WAV: &[u8] = include_bytes!("../assets/sounds/party.wav");
const GUILD_WAV: &[u8] = include_bytes!("../assets/sounds/guild.wav");
const PM_WAV: &[u8] = include_bytes!("../assets/sounds/pm.wav");
const TRADE_WAV: &[u8] = include_bytes!("../assets/sounds/trade.wav");

#[cfg(realmhound_official_sounds)]
const DIMITUS_ALERT: &[u8] = include_bytes!("../assets/sounds/dimitus.mp3");
#[cfg(not(realmhound_official_sounds))]
const DIMITUS_ALERT: &[u8] = KEYPOP_WAV;

#[cfg(realmhound_official_sounds)]
const BAD_MOD_WARNING: &[u8] = include_bytes!("../assets/sounds/bad_mod_warning.mp3");
#[cfg(not(realmhound_official_sounds))]
const BAD_MOD_WARNING: &[u8] = REDBAG_WAV;

#[cfg(realmhound_official_sounds)]
const EVENT_VETERAN: &[u8] = include_bytes!("../assets/sounds/events/veteran.mp3");
#[cfg(not(realmhound_official_sounds))]
const EVENT_VETERAN: &[u8] = PARTY_WAV;

#[cfg(realmhound_official_sounds)]
const EVENT_ADEPT: &[u8] = include_bytes!("../assets/sounds/events/adept.mp3");
#[cfg(not(realmhound_official_sounds))]
const EVENT_ADEPT: &[u8] = GUILD_WAV;

#[cfg(realmhound_official_sounds)]
const EVENT_SEASONAL: &[u8] = include_bytes!("../assets/sounds/events/seasonal.mp3");
#[cfg(not(realmhound_official_sounds))]
const EVENT_SEASONAL: &[u8] = PM_WAV;

#[cfg(realmhound_official_sounds)]
const EVENT_ALIEN_WAVE: &[u8] = include_bytes!("../assets/sounds/events/alien_wave.mp3");
#[cfg(not(realmhound_official_sounds))]
const EVENT_ALIEN_WAVE: &[u8] = BLUEBAG_WAV;

#[cfg(realmhound_official_sounds)]
const EVENT_UFO: &[u8] = include_bytes!("../assets/sounds/events/ufo_.mp3");
#[cfg(not(realmhound_official_sounds))]
const EVENT_UFO: &[u8] = ORANGEBAG_WAV;

#[cfg(realmhound_official_sounds)]
const EVENT_CALBRIK: &[u8] = include_bytes!("../assets/sounds/events/calbrik.mp3");
#[cfg(not(realmhound_official_sounds))]
const EVENT_CALBRIK: &[u8] = TRADE_WAV;

#[cfg(realmhound_official_sounds)]
const EVENT_ENCHANT: &[u8] = include_bytes!("../assets/sounds/enchants/money.mp3");
#[cfg(not(realmhound_official_sounds))]
const EVENT_ENCHANT: &[u8] = GOLDBAG_WAV;

/// Sound types that can be played.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(dead_code)] // Social sounds not yet implemented
pub enum SoundType {
    WhiteBag,
    OrangeBag,
    RedBag,
    BlueBag,
    GoldBag,
    EggBag,
    KeyPop,
    /// Key pop in a Rookie-tier dungeon (own volume/custom sound).
    KeyPopRookie,
    /// Key pop in an Adept-tier dungeon (own volume/custom sound).
    KeyPopAdept,
    /// Key pop in an Expert-tier dungeon (own volume/custom sound).
    KeyPopExpert,
    /// Key pop in an Exaltation-tier dungeon (own volume/custom sound).
    KeyPopExaltation,
    Party,
    Guild,
    Pm,
    Trade,
    /// Plays when entering a Dimitus dungeon.
    DimitusAlert,
    /// Plays when entering a dungeon with a dangerous (red-outline) modifier
    /// set.
    BadModWarning,
}

impl SoundType {
    /// Get the embedded default audio bytes for this sound type.
    fn audio_data(&self) -> &'static [u8] {
        match self {
            Self::WhiteBag => WHITEBAG_WAV,
            Self::OrangeBag => ORANGEBAG_WAV,
            Self::RedBag => REDBAG_WAV,
            Self::BlueBag => BLUEBAG_WAV,
            Self::GoldBag => GOLDBAG_WAV,
            Self::EggBag => EGGBAG_WAV,
            Self::KeyPop => KEYPOP_WAV,
            Self::KeyPopRookie => KEYPOP_WAV,
            Self::KeyPopAdept => KEYPOP_WAV,
            Self::KeyPopExpert => KEYPOP_WAV,
            Self::KeyPopExaltation => KEYPOP_WAV,
            Self::Party => PARTY_WAV,
            Self::Guild => GUILD_WAV,
            Self::Pm => PM_WAV,
            Self::Trade => TRADE_WAV,
            Self::DimitusAlert => DIMITUS_ALERT,
            Self::BadModWarning => BAD_MOD_WARNING,
        }
    }

    /// Check if this sound is enabled in the settings.
    pub fn is_enabled(&self, settings: &SoundSettings) -> bool {
        match self {
            Self::WhiteBag => settings.whitebag,
            Self::OrangeBag => settings.orangebag,
            Self::RedBag => settings.redbag,
            Self::BlueBag => settings.bluebag,
            Self::GoldBag => settings.goldbag,
            Self::EggBag => settings.eggbag,
            Self::KeyPop => settings.keypop,
            Self::KeyPopRookie => settings.keypop && settings.keypop_tiers.rookie,
            Self::KeyPopAdept => settings.keypop && settings.keypop_tiers.adept,
            Self::KeyPopExpert => settings.keypop && settings.keypop_tiers.expert,
            Self::KeyPopExaltation => settings.keypop && settings.keypop_tiers.exaltation,
            Self::Party => settings.party,
            Self::Guild => settings.guild,
            Self::Pm => settings.pm,
            Self::Trade => settings.trade,
            Self::DimitusAlert => settings.dimitus_dungeon,
            Self::BadModWarning => settings.bad_mod_warning,
        }
    }

    /// Get the sound type for a loot bag type.
    pub fn from_bag_type(bag_type: LootBagType) -> Option<Self> {
        match bag_type {
            LootBagType::White | LootBagType::BoostedWhite => Some(Self::WhiteBag),
            LootBagType::Orange | LootBagType::BoostedOrange => Some(Self::OrangeBag),
            LootBagType::Red | LootBagType::BoostedRed => Some(Self::RedBag),
            LootBagType::Blue | LootBagType::BoostedBlue => Some(Self::BlueBag),
            LootBagType::Gold | LootBagType::BoostedGold => Some(Self::GoldBag),
            LootBagType::Egg | LootBagType::BoostedEgg => Some(Self::EggBag),
            // No sounds for lower tier bags
            _ => None,
        }
    }

    /// Map a key-pop difficulty tier to its dedicated sound type so per-tier
    /// volume and custom-sound overrides apply. Unknown-tier pops (`None`) fall
    /// back to the base [`Self::KeyPop`].
    pub fn key_pop_for_tier(tier: Option<realmhound_core::assets::KeyPopTier>) -> Self {
        use realmhound_core::assets::KeyPopTier;
        match tier {
            Some(KeyPopTier::Rookie) => Self::KeyPopRookie,
            Some(KeyPopTier::Adept) => Self::KeyPopAdept,
            Some(KeyPopTier::Expert) => Self::KeyPopExpert,
            Some(KeyPopTier::Exaltation) => Self::KeyPopExaltation,
            None => Self::KeyPop,
        }
    }

    /// The settings key used to identify this sound in `SoundSettings::custom_sounds`.
    pub fn settings_key(&self) -> &'static str {
        match self {
            Self::WhiteBag => "whitebag",
            Self::OrangeBag => "orangebag",
            Self::RedBag => "redbag",
            Self::BlueBag => "bluebag",
            Self::GoldBag => "goldbag",
            Self::EggBag => "eggbag",
            Self::KeyPop => "keypop",
            Self::KeyPopRookie => "keypop_rookie",
            Self::KeyPopAdept => "keypop_adept",
            Self::KeyPopExpert => "keypop_expert",
            Self::KeyPopExaltation => "keypop_exaltation",
            Self::Party => "party",
            Self::Guild => "guild",
            Self::Pm => "pm",
            Self::Trade => "trade",
            Self::DimitusAlert => "dimitus_alert",
            Self::BadModWarning => "bad_mod_warning",
        }
    }
}

/// Realm-event notification sounds. Unlike [`SoundType`] these play at a
/// per-event volume (scaled by master) and support arbitrary per-boss custom
/// overrides keyed by a caller-supplied string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventSound {
    Veteran,
    Adept,
    Seasonal,
    AlienWave,
    Ufo,
    Calbrik,
    /// Enchantment notification (tiered and unique/awakened categories).
    Enchant,
}

impl EventSound {
    /// Embedded default audio bytes for this event category.
    pub fn audio_data(&self) -> &'static [u8] {
        match self {
            Self::Veteran => EVENT_VETERAN,
            Self::Adept => EVENT_ADEPT,
            Self::Seasonal => EVENT_SEASONAL,
            Self::AlienWave => EVENT_ALIEN_WAVE,
            Self::Ufo => EVENT_UFO,
            Self::Calbrik => EVENT_CALBRIK,
            Self::Enchant => EVENT_ENCHANT,
        }
    }
}

/// Sound engine for playing notification sounds.
///
/// Recreates the audio output stream on each play so that it always targets
/// the current system default device.
pub struct SoundEngine {
    /// Current volume (0.0 to 1.0).
    volume: f32,
}

#[cfg(feature = "latency-diagnostics")]
#[derive(Default)]
struct PlaybackLatencyDiagnostics {
    health_started: Option<std::time::Instant>,
    health_play_commands: u64,
    health_bag_commands: u64,
    health_event_commands: u64,
    health_volume_commands: u64,
    health_stale_drops: u64,
    health_full_drops: u64,
    health_delayed_commands: u64,
    health_peak_queue: std::time::Duration,
    health_playback_preparations: u64,
    health_peak_ready: std::time::Duration,
    health_peak_stream: std::time::Duration,
    health_peak_sink: std::time::Duration,
    health_peak_prepare: std::time::Duration,
    total: u64,
    count_since_warn: u64,
    peak_ready: std::time::Duration,
    peak_stream: std::time::Duration,
    peak_sink: std::time::Duration,
    peak_prepare: std::time::Duration,
    last_warn: Option<std::time::Instant>,
}

#[cfg(feature = "latency-diagnostics")]
struct PlaybackLatencyWarning {
    total: u64,
    count_since_warn: u64,
    peak_ready: std::time::Duration,
    peak_stream: std::time::Duration,
    peak_sink: std::time::Duration,
    peak_prepare: std::time::Duration,
}

#[cfg(feature = "latency-diagnostics")]
struct AudioHealthSummary {
    window: std::time::Duration,
    play_commands: u64,
    bag_commands: u64,
    event_commands: u64,
    volume_commands: u64,
    stale_drops: u64,
    full_drops: u64,
    delayed_commands: u64,
    peak_queue: std::time::Duration,
    playback_preparations: u64,
    peak_ready: std::time::Duration,
    peak_stream: std::time::Duration,
    peak_sink: std::time::Duration,
    peak_prepare: std::time::Duration,
}

#[cfg(feature = "latency-diagnostics")]
impl PlaybackLatencyDiagnostics {
    fn record_slow_playback(
        &mut self,
        now: std::time::Instant,
        ready: std::time::Duration,
        stream: std::time::Duration,
        sink: std::time::Duration,
        prepare: std::time::Duration,
    ) -> Option<PlaybackLatencyWarning> {
        self.total = self.total.saturating_add(1);
        self.count_since_warn = self.count_since_warn.saturating_add(1);
        self.peak_ready = self.peak_ready.max(ready);
        self.peak_stream = self.peak_stream.max(stream);
        self.peak_sink = self.peak_sink.max(sink);
        self.peak_prepare = self.peak_prepare.max(prepare);
        if self
            .last_warn
            .is_some_and(|last| now.duration_since(last) < std::time::Duration::from_secs(2))
        {
            return None;
        }
        self.last_warn = Some(now);
        let warning = PlaybackLatencyWarning {
            total: self.total,
            count_since_warn: self.count_since_warn,
            peak_ready: self.peak_ready,
            peak_stream: self.peak_stream,
            peak_sink: self.peak_sink,
            peak_prepare: self.peak_prepare,
        };
        self.count_since_warn = 0;
        self.peak_ready = std::time::Duration::ZERO;
        self.peak_stream = std::time::Duration::ZERO;
        self.peak_sink = std::time::Duration::ZERO;
        self.peak_prepare = std::time::Duration::ZERO;
        Some(warning)
    }

    fn roll_health_window(&mut self, now: std::time::Instant) -> Option<AudioHealthSummary> {
        let summary = self.health_started.and_then(|started| {
            let window = now.duration_since(started);
            (window >= std::time::Duration::from_secs(30)).then(|| AudioHealthSummary {
                window,
                play_commands: self.health_play_commands,
                bag_commands: self.health_bag_commands,
                event_commands: self.health_event_commands,
                volume_commands: self.health_volume_commands,
                stale_drops: self.health_stale_drops,
                full_drops: self.health_full_drops,
                delayed_commands: self.health_delayed_commands,
                peak_queue: self.health_peak_queue,
                playback_preparations: self.health_playback_preparations,
                peak_ready: self.health_peak_ready,
                peak_stream: self.health_peak_stream,
                peak_sink: self.health_peak_sink,
                peak_prepare: self.health_peak_prepare,
            })
        });
        if summary.is_none() {
            self.health_started.get_or_insert(now);
            return None;
        }
        self.health_started = Some(now);
        self.health_play_commands = 0;
        self.health_bag_commands = 0;
        self.health_event_commands = 0;
        self.health_volume_commands = 0;
        self.health_stale_drops = 0;
        self.health_full_drops = 0;
        self.health_delayed_commands = 0;
        self.health_peak_queue = std::time::Duration::ZERO;
        self.health_playback_preparations = 0;
        self.health_peak_ready = std::time::Duration::ZERO;
        self.health_peak_stream = std::time::Duration::ZERO;
        self.health_peak_sink = std::time::Duration::ZERO;
        self.health_peak_prepare = std::time::Duration::ZERO;
        summary
    }

    fn record_command_health(
        &mut self,
        now: std::time::Instant,
        command_kind: &'static str,
        queue_age: std::time::Duration,
        stale: bool,
    ) -> Option<AudioHealthSummary> {
        let summary = self.roll_health_window(now);
        match command_kind {
            "play" => self.health_play_commands += 1,
            "play_for_bag" => self.health_bag_commands += 1,
            "play_event" => self.health_event_commands += 1,
            "set_volume" => self.health_volume_commands += 1,
            _ => {}
        }
        if stale {
            self.health_stale_drops += 1;
        }
        if queue_age >= std::time::Duration::from_millis(250) {
            self.health_delayed_commands += 1;
            self.health_peak_queue = self.health_peak_queue.max(queue_age);
        }
        summary
    }

    fn record_full_health(&mut self, now: std::time::Instant) -> Option<AudioHealthSummary> {
        let summary = self.roll_health_window(now);
        self.health_full_drops += 1;
        summary
    }

    fn record_playback_health(
        &mut self,
        now: std::time::Instant,
        ready: std::time::Duration,
        stream: std::time::Duration,
        sink: std::time::Duration,
        prepare: std::time::Duration,
    ) -> Option<AudioHealthSummary> {
        let summary = self.roll_health_window(now);
        self.health_playback_preparations += 1;
        self.health_peak_ready = self.health_peak_ready.max(ready);
        self.health_peak_stream = self.health_peak_stream.max(stream);
        self.health_peak_sink = self.health_peak_sink.max(sink);
        self.health_peak_prepare = self.health_peak_prepare.max(prepare);
        summary
    }
}

#[cfg(feature = "latency-diagnostics")]
static PLAYBACK_LATENCY: std::sync::Mutex<PlaybackLatencyDiagnostics> =
    std::sync::Mutex::new(PlaybackLatencyDiagnostics {
        health_started: None,
        health_play_commands: 0,
        health_bag_commands: 0,
        health_event_commands: 0,
        health_volume_commands: 0,
        health_stale_drops: 0,
        health_full_drops: 0,
        health_delayed_commands: 0,
        health_peak_queue: std::time::Duration::ZERO,
        health_playback_preparations: 0,
        health_peak_ready: std::time::Duration::ZERO,
        health_peak_stream: std::time::Duration::ZERO,
        health_peak_sink: std::time::Duration::ZERO,
        health_peak_prepare: std::time::Duration::ZERO,
        total: 0,
        count_since_warn: 0,
        peak_ready: std::time::Duration::ZERO,
        peak_stream: std::time::Duration::ZERO,
        peak_sink: std::time::Duration::ZERO,
        peak_prepare: std::time::Duration::ZERO,
        last_warn: None,
    });

#[cfg(feature = "latency-diagnostics")]
fn log_audio_health(summary: AudioHealthSummary) {
    tracing::info!(
        "[LATENCY][AUDIO] health window_ms={} commands_play={} commands_bag={} \
         commands_event={} commands_volume={} stale_drops={} channel_full_drops={} \
         delayed_commands={} peak_queue_ms={} playback_preparations={} \
         peak_playback_ready_ms={} peak_stream_open_ms={} peak_sink_create_ms={} \
         peak_decode_read_prepare_ms={}",
        summary.window.as_millis(),
        summary.play_commands,
        summary.bag_commands,
        summary.event_commands,
        summary.volume_commands,
        summary.stale_drops,
        summary.full_drops,
        summary.delayed_commands,
        summary.peak_queue.as_millis(),
        summary.playback_preparations,
        summary.peak_ready.as_millis(),
        summary.peak_stream.as_millis(),
        summary.peak_sink.as_millis(),
        summary.peak_prepare.as_millis()
    );
}

#[cfg(feature = "latency-diagnostics")]
fn update_audio_health(
    update: impl FnOnce(
        &mut PlaybackLatencyDiagnostics,
        std::time::Instant,
    ) -> Option<AudioHealthSummary>,
) {
    let now = std::time::Instant::now();
    let summary = {
        let mut diagnostics = PLAYBACK_LATENCY.lock().unwrap_or_else(|e| e.into_inner());
        update(&mut diagnostics, now)
    };
    if let Some(summary) = summary {
        log_audio_health(summary);
    }
}

#[cfg(feature = "latency-diagnostics")]
pub(crate) fn record_audio_command(
    command_kind: &'static str,
    queue_age: std::time::Duration,
    stale: bool,
) {
    update_audio_health(|diagnostics, now| {
        diagnostics.record_command_health(now, command_kind, queue_age, stale)
    });
}

#[cfg(feature = "latency-diagnostics")]
pub(crate) fn record_audio_channel_full() {
    update_audio_health(|diagnostics, now| diagnostics.record_full_health(now));
}

#[cfg(feature = "latency-diagnostics")]
fn record_playback_latency(
    playback_ready: std::time::Duration,
    stream: std::time::Duration,
    sink: std::time::Duration,
    prepare: std::time::Duration,
) {
    let now = std::time::Instant::now();
    let (warning, health) = {
        let mut diagnostics = PLAYBACK_LATENCY.lock().unwrap_or_else(|e| e.into_inner());
        let health = diagnostics.record_playback_health(now, playback_ready, stream, sink, prepare);
        let warning = (playback_ready >= std::time::Duration::from_millis(250))
            .then(|| diagnostics.record_slow_playback(now, playback_ready, stream, sink, prepare))
            .flatten();
        (warning, health)
    };
    if let Some(warning) = warning {
        tracing::warn!(
            "[LATENCY][AUDIO] stage=playback_ready count_since_warn={} total={} \
             peak_playback_ready_ms={} peak_stream_open_ms={} peak_sink_create_ms={} \
             peak_decode_read_prepare_ms={}",
            warning.count_since_warn,
            warning.total,
            warning.peak_ready.as_millis(),
            warning.peak_stream.as_millis(),
            warning.peak_sink.as_millis(),
            warning.peak_prepare.as_millis()
        );
    }
    if let Some(health) = health {
        log_audio_health(health);
    }
}

/// Get the custom sounds directory: `%LOCALAPPDATA%\RealmHound\sounds\custom\`.
pub fn custom_sounds_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("RealmHound").join("sounds").join("custom"))
}

/// Maximum custom sound file size (2 MB). Prevents large files from causing
/// allocation issues on every play.
pub const MAX_CUSTOM_SOUND_BYTES: u64 = 2 * 1024 * 1024;

/// Resolve the full path for a custom sound file (if configured and file exists).
pub fn resolve_custom_sound(settings: &SoundSettings, sound: SoundType) -> Option<PathBuf> {
    resolve_custom_sound_key(settings, sound.settings_key())
}

/// Resolve the custom sound file for an arbitrary `custom_sounds` key (used by
/// realm-event notifications, which key overrides by boss id rather than a
/// fixed [`SoundType`]).
pub fn resolve_custom_sound_key(settings: &SoundSettings, key: &str) -> Option<PathBuf> {
    let filename = settings.custom_sounds.get(key)?;
    // Reject filenames with path separators or parent components
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        tracing::warn!(
            "[SOUND] Rejecting unsafe custom sound filename: {}",
            filename
        );
        return None;
    }
    let dir = custom_sounds_dir()?;
    let path = dir.join(filename);
    if path.is_file() {
        Some(path)
    } else {
        tracing::warn!(
            "[SOUND] Custom sound file missing for key {}: {}",
            key,
            path.display()
        );
        None
    }
}

impl SoundEngine {
    /// Create a new sound engine.
    ///
    /// Always succeeds -- device availability is checked per-play so that
    /// hot-plugged devices are picked up without restart.
    pub fn new() -> Self {
        if OutputStream::try_default().is_ok() {
            tracing::info!("[SOUND] Audio output available");
        } else {
            tracing::warn!("[SOUND] No audio output device at startup (will retry per-play)");
        }
        Self { volume: 0.8 }
    }

    /// Set the master volume (0.0 to 1.0).
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    /// Get the current volume.
    #[allow(dead_code)] // May be used in future UI
    pub fn volume(&self) -> f32 {
        self.volume
    }

    /// Play a sound at an explicit volume (already scaled by the caller's
    /// master multiplier), using a custom file path if provided, falling back
    /// to the embedded default on any error.
    pub fn play(
        &self,
        sound: SoundType,
        custom_path: Option<PathBuf>,
        volume: f32,
        #[cfg(feature = "latency-diagnostics")] dispatched_at: std::time::Instant,
    ) {
        let volume = volume.clamp(0.0, 1.0);
        std::thread::Builder::new()
            .name("realmhound-audio-play".into())
            .spawn(move || {
                #[cfg(feature = "latency-diagnostics")]
                let stream_started = std::time::Instant::now();
                let (_stream, stream_handle) = match OutputStream::try_default() {
                    Ok(pair) => pair,
                    Err(e) => {
                        tracing::warn!("[SOUND] Failed to open audio output: {}", e);
                        return;
                    }
                };
                #[cfg(feature = "latency-diagnostics")]
                let stream_ms = stream_started.elapsed();

                #[cfg(feature = "latency-diagnostics")]
                let sink_started = std::time::Instant::now();
                let sink = match Sink::try_new(&stream_handle) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("[SOUND] Failed to create audio sink: {}", e);
                        return;
                    }
                };
                #[cfg(feature = "latency-diagnostics")]
                let sink_ms = sink_started.elapsed();
                sink.set_volume(volume);

                // Try custom file first, fall back to embedded default.
                #[cfg(feature = "latency-diagnostics")]
                let prepare_started = std::time::Instant::now();
                let played_custom = custom_path.is_some_and(|path| match fs::read(&path) {
                    Ok(data) => match Decoder::new(Cursor::new(data)) {
                        Ok(source) => {
                            sink.append(source);
                            true
                        }
                        Err(e) => {
                            tracing::warn!(
                                "[SOUND] Failed to decode custom {:?} ({}): {}",
                                sound,
                                path.display(),
                                e
                            );
                            false
                        }
                    },
                    Err(e) => {
                        tracing::warn!(
                            "[SOUND] Failed to read custom {:?} ({}): {}",
                            sound,
                            path.display(),
                            e
                        );
                        false
                    }
                });

                if !played_custom {
                    // Zero-copy path for embedded defaults
                    match Decoder::new(Cursor::new(sound.audio_data())) {
                        Ok(source) => sink.append(source),
                        Err(e) => {
                            tracing::warn!("[SOUND] Failed to decode embedded {:?}: {}", sound, e);
                            return;
                        }
                    }
                }
                #[cfg(feature = "latency-diagnostics")]
                {
                    let prepare_ms = prepare_started.elapsed();
                    let playback_ready_ms = dispatched_at.elapsed();
                    record_playback_latency(playback_ready_ms, stream_ms, sink_ms, prepare_ms);
                }

                tracing::trace!(
                    "[SOUND] Playing {:?} at volume {:.0}%{}",
                    sound,
                    volume * 100.0,
                    if played_custom { " (custom)" } else { "" }
                );
                sink.sleep_until_end();
            })
            .ok();
    }

    /// Play a realm-event sound at an explicit volume (already scaled by the
    /// caller's master multiplier). Tries the custom file, falls back to the
    /// embedded default. Independent of the engine's master `volume` field.
    pub fn play_at(
        &self,
        sound: EventSound,
        custom_path: Option<PathBuf>,
        volume: f32,
        #[cfg(feature = "latency-diagnostics")] dispatched_at: std::time::Instant,
    ) {
        let volume = volume.clamp(0.0, 1.0);
        std::thread::Builder::new()
            .name("realmhound-audio-play".into())
            .spawn(move || {
                #[cfg(feature = "latency-diagnostics")]
                let stream_started = std::time::Instant::now();
                let (_stream, stream_handle) = match OutputStream::try_default() {
                    Ok(pair) => pair,
                    Err(e) => {
                        tracing::warn!("[SOUND] Failed to open audio output: {}", e);
                        return;
                    }
                };
                #[cfg(feature = "latency-diagnostics")]
                let stream_ms = stream_started.elapsed();

                #[cfg(feature = "latency-diagnostics")]
                let sink_started = std::time::Instant::now();
                let sink = match Sink::try_new(&stream_handle) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("[SOUND] Failed to create audio sink: {}", e);
                        return;
                    }
                };
                #[cfg(feature = "latency-diagnostics")]
                let sink_ms = sink_started.elapsed();
                sink.set_volume(volume);

                #[cfg(feature = "latency-diagnostics")]
                let prepare_started = std::time::Instant::now();
                let played_custom = custom_path.is_some_and(|path| match fs::read(&path) {
                    Ok(data) => match Decoder::new(Cursor::new(data)) {
                        Ok(source) => {
                            sink.append(source);
                            true
                        }
                        Err(e) => {
                            tracing::warn!(
                                "[SOUND] Failed to decode custom event {:?} ({}): {}",
                                sound,
                                path.display(),
                                e
                            );
                            false
                        }
                    },
                    Err(e) => {
                        tracing::warn!(
                            "[SOUND] Failed to read custom event {:?} ({}): {}",
                            sound,
                            path.display(),
                            e
                        );
                        false
                    }
                });

                if !played_custom {
                    match Decoder::new(Cursor::new(sound.audio_data())) {
                        Ok(source) => sink.append(source),
                        Err(e) => {
                            tracing::warn!(
                                "[SOUND] Failed to decode embedded event {:?}: {}",
                                sound,
                                e
                            );
                            return;
                        }
                    }
                }
                #[cfg(feature = "latency-diagnostics")]
                {
                    let prepare_ms = prepare_started.elapsed();
                    let playback_ready_ms = dispatched_at.elapsed();
                    record_playback_latency(playback_ready_ms, stream_ms, sink_ms, prepare_ms);
                }

                tracing::trace!(
                    "[SOUND] Playing event {:?} at volume {:.0}%{}",
                    sound,
                    volume * 100.0,
                    if played_custom { " (custom)" } else { "" }
                );
                sink.sleep_until_end();
            })
            .ok();
    }
}

#[cfg(all(test, feature = "latency-diagnostics"))]
mod latency_tests {
    use super::*;

    #[test]
    fn playback_latency_aggregates_and_resets_window_peaks() {
        let now = std::time::Instant::now();
        let mut diagnostics = PlaybackLatencyDiagnostics::default();
        let first = diagnostics
            .record_slow_playback(
                now,
                std::time::Duration::from_millis(300),
                std::time::Duration::from_millis(100),
                std::time::Duration::from_millis(20),
                std::time::Duration::from_millis(30),
            )
            .unwrap();
        assert_eq!(first.count_since_warn, 1);
        assert!(diagnostics
            .record_slow_playback(
                now + std::time::Duration::from_millis(100),
                std::time::Duration::from_millis(900),
                std::time::Duration::from_millis(400),
                std::time::Duration::from_millis(40),
                std::time::Duration::from_millis(80),
            )
            .is_none());
        let summary = diagnostics
            .record_slow_playback(
                now + std::time::Duration::from_secs(2),
                std::time::Duration::from_millis(500),
                std::time::Duration::from_millis(200),
                std::time::Duration::from_millis(30),
                std::time::Duration::from_millis(50),
            )
            .unwrap();
        assert_eq!(summary.count_since_warn, 2);
        assert_eq!(summary.total, 3);
        assert_eq!(summary.peak_ready, std::time::Duration::from_millis(900));
        assert_eq!(summary.peak_stream, std::time::Duration::from_millis(400));
        assert_eq!(diagnostics.peak_ready, std::time::Duration::ZERO);
    }

    #[test]
    fn audio_health_rolls_before_atomic_command_and_resets() {
        let now = std::time::Instant::now();
        let mut diagnostics = PlaybackLatencyDiagnostics::default();
        assert!(diagnostics
            .record_command_health(now, "play", std::time::Duration::from_millis(100), false,)
            .is_none());
        assert!(diagnostics
            .record_playback_health(
                now + std::time::Duration::from_millis(1),
                std::time::Duration::from_millis(600),
                std::time::Duration::from_millis(200),
                std::time::Duration::from_millis(30),
                std::time::Duration::from_millis(50),
            )
            .is_none());

        let first = diagnostics
            .record_command_health(
                now + std::time::Duration::from_secs(33),
                "play_for_bag",
                std::time::Duration::from_millis(500),
                true,
            )
            .unwrap();
        assert_eq!(first.window, std::time::Duration::from_secs(33));
        assert_eq!(first.play_commands, 1);
        assert_eq!(first.bag_commands, 0);
        assert_eq!(first.stale_drops, 0);
        assert_eq!(first.playback_preparations, 1);
        assert_eq!(first.peak_ready, std::time::Duration::from_millis(600));
        assert_eq!(diagnostics.health_play_commands, 0);
        assert_eq!(diagnostics.health_bag_commands, 1);
        assert_eq!(diagnostics.health_stale_drops, 1);
        assert_eq!(diagnostics.health_delayed_commands, 1);

        let second = diagnostics
            .record_command_health(
                now + std::time::Duration::from_secs(64),
                "set_volume",
                std::time::Duration::ZERO,
                false,
            )
            .unwrap();
        assert_eq!(second.window, std::time::Duration::from_secs(31));
        assert_eq!(second.bag_commands, 1);
        assert_eq!(second.stale_drops, 1);
        assert_eq!(second.delayed_commands, 1);
        assert_eq!(second.peak_queue, std::time::Duration::from_millis(500));
        assert_eq!(diagnostics.health_volume_commands, 1);
    }
}

//! In-app puffin profiler overlay.
//!
//! Everything here is compiled out unless the `profiling` feature is enabled.
//! Build with `cargo run -p RealmHound --release --features profiling`. Then:
//!
//! - Press `F12` in the app to toggle the live profiler window.
//! - Frame-time stats (mean / p50 / p95 / max) are written to the tracing log
//!   every few seconds - see `%LOCALAPPDATA%\RealmHound\logs\`.
//! - A `puffin_http` server runs on `127.0.0.1:8585`; record/inspect offline
//!   with `puffin_viewer --url 127.0.0.1:8585`.

use eframe::egui;

#[cfg(feature = "profiling")]
mod imp {
    use super::egui;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::OnceLock;
    use std::time::Instant;

    static WINDOW_OPEN: AtomicBool = AtomicBool::new(false);
    static INITIALIZED: AtomicBool = AtomicBool::new(false);

    // Latest cache stats (updated each frame, logged every 5s)
    static CACHE_GLOW: AtomicUsize = AtomicUsize::new(0);
    static CACHE_DYE: AtomicUsize = AtomicUsize::new(0);
    static CACHE_CHAR_DYE: AtomicUsize = AtomicUsize::new(0);
    static CACHE_BYTES: AtomicUsize = AtomicUsize::new(0);

    pub fn update_cache_stats(glow: usize, dye: usize, char_dye: usize, bytes: usize) {
        CACHE_GLOW.store(glow, Ordering::Relaxed);
        CACHE_DYE.store(dye, Ordering::Relaxed);
        CACHE_CHAR_DYE.store(char_dye, Ordering::Relaxed);
        CACHE_BYTES.store(bytes, Ordering::Relaxed);
    }

    /// Kept alive for the process lifetime so `puffin_viewer` can connect.
    static HTTP_SERVER: OnceLock<puffin_http::Server> = OnceLock::new();

    /// Address the puffin HTTP server listens on. Connect a recorder with
    /// `puffin_viewer --url 127.0.0.1:8585`.
    const SERVER_ADDR: &str = "127.0.0.1:8585";

    /// Frame-time accumulator so we can write periodic stats to the tracing log
    /// (processable without any external viewer). Samples are the *work* time
    /// spent inside `update()` (not the wall-clock gap between frames, which is
    /// dominated by egui sleeping while idle), grouped by active tab so stalls
    /// can be attributed to a specific panel.
    struct FrameStats {
        window_start: Instant,
        frame_start: Option<Instant>,
        cur_tab: &'static str,
        by_tab: std::collections::HashMap<&'static str, Vec<f32>>,
        causes: std::collections::HashMap<String, u32>,
        draw_calls: Vec<u32>,
        outlined_sprites: Vec<u32>,
    }

    fn frame_stats() -> &'static std::sync::Mutex<FrameStats> {
        static STATS: OnceLock<std::sync::Mutex<FrameStats>> = OnceLock::new();
        STATS.get_or_init(|| {
            std::sync::Mutex::new(FrameStats {
                window_start: Instant::now(),
                frame_start: None,
                cur_tab: "?",
                by_tab: std::collections::HashMap::new(),
                causes: std::collections::HashMap::new(),
                draw_calls: Vec::new(),
                outlined_sprites: Vec::new(),
            })
        })
    }

    /// How often to flush frame-time stats to the log.
    const LOG_INTERVAL_SECS: f32 = 5.0;

    /// Called at the very start of `update()`: stamp the frame start + tab.
    fn begin_frame(tab: &'static str) {
        if let Ok(mut s) = frame_stats().lock() {
            s.frame_start = Some(Instant::now());
            s.cur_tab = tab;
        }
    }

    /// Called at the very end of `update()`: record how long the frame's work
    /// took and flush per-tab stats to the log every `LOG_INTERVAL_SECS`.
    pub fn end_frame(ctx: &egui::Context) {
        let Ok(mut s) = frame_stats().lock() else {
            return;
        };
        let now = Instant::now();
        if let Some(start) = s.frame_start.take() {
            let work_ms = (now - start).as_secs_f32() * 1000.0;
            let tab = s.cur_tab;
            s.by_tab.entry(tab).or_default().push(work_ms);
        }

        // Collect sprite draw-call stats for this frame
        let (dc, os) = crate::rendering::sprite_renderer::take_draw_call_count();
        s.draw_calls.push(dc);
        s.outlined_sprites.push(os);

        // Record what requested a repaint for the next pass so a continuous
        // repaint source can be attributed to a file:line.
        for cause in ctx.repaint_causes() {
            *s.causes.entry(format!("{cause}")).or_insert(0) += 1;
        }

        if (now - s.window_start).as_secs_f32() < LOG_INTERVAL_SECS {
            return;
        }
        let total: usize = s.by_tab.values().map(Vec::len).sum();
        if total == 0 {
            s.window_start = now;
            return;
        }

        // Log one line per tab that had frames this window, busiest first.
        let mut tabs: Vec<(&'static str, Vec<f32>)> = s.by_tab.drain().collect();
        tabs.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
        for (tab, mut samples) in tabs {
            samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let n = samples.len();
            let mean = samples.iter().sum::<f32>() / n as f32;
            let p50 = samples[n / 2];
            let p95 = samples[(n * 95 / 100).min(n - 1)];
            let max = samples[n - 1];
            tracing::info!(
                "[PROFILING] tab={tab} frames={n} work: mean={mean:.2}ms p50={p50:.2}ms p95={p95:.2}ms max={max:.2}ms"
            );
        }

        // Log sprite draw-call stats
        if !s.draw_calls.is_empty() {
            let n = s.draw_calls.len();
            let dc_mean = s.draw_calls.iter().sum::<u32>() as f32 / n as f32;
            let dc_max = *s.draw_calls.iter().max().unwrap_or(&0);
            let os_mean = s.outlined_sprites.iter().sum::<u32>() as f32 / n as f32;
            let os_max = *s.outlined_sprites.iter().max().unwrap_or(&0);
            tracing::info!(
                "[PROFILING] sprites: draw_calls mean={dc_mean:.0} max={dc_max} | outlined mean={os_mean:.0} max={os_max} (each=9 draws)"
            );
            s.draw_calls.clear();
            s.outlined_sprites.clear();
        }

        // Log cache memory stats
        let glow = CACHE_GLOW.load(Ordering::Relaxed);
        let dye = CACHE_DYE.load(Ordering::Relaxed);
        let char_dye = CACHE_CHAR_DYE.load(Ordering::Relaxed);
        let bytes = CACHE_BYTES.load(Ordering::Relaxed);
        let mb = bytes as f64 / (1024.0 * 1024.0);
        tracing::info!(
            "[PROFILING] caches: baked={glow} dye={dye} char_dye={char_dye} retained={mb:.1}MB"
        );

        // Report the top repaint-request sources this window so a continuous
        // repaint (idle CPU) can be traced to its origin.
        let mut causes: Vec<(String, u32)> = s.causes.drain().collect();
        causes.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
        let top: Vec<String> = causes
            .iter()
            .take(6)
            .map(|(cause, c)| format!("{c}x {cause}"))
            .collect();
        if !top.is_empty() {
            tracing::info!("[PROFILING] repaint causes: {}", top.join(" | "));
        }
        s.window_start = now;
    }

    /// Mark a new profiling frame, handle the F12 toggle, and draw the profiler
    /// window when it is open. Call once at the top of the egui update.
    pub fn frame(ctx: &egui::Context, active_tab: &'static str) {
        if !INITIALIZED.swap(true, Ordering::Relaxed) {
            puffin::set_scopes_on(true);
            match puffin_http::Server::new(SERVER_ADDR) {
                Ok(server) => {
                    let _ = HTTP_SERVER.set(server);
                    tracing::info!(
                        "[PROFILING] puffin enabled - F12 for overlay; work-time stats logged every {LOG_INTERVAL_SECS:.0}s; puffin_viewer --url {SERVER_ADDR}"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "[PROFILING] puffin HTTP server failed to start on {SERVER_ADDR}: {e}"
                    );
                }
            }
        }

        puffin::GlobalProfiler::lock().new_frame();
        begin_frame(active_tab);

        if ctx.input(|i| i.key_pressed(egui::Key::F12)) {
            let now = !WINDOW_OPEN.load(Ordering::Relaxed);
            WINDOW_OPEN.store(now, Ordering::Relaxed);
        }

        if WINDOW_OPEN.load(Ordering::Relaxed) {
            let still_open = puffin_egui::profiler_window(ctx);
            if !still_open {
                WINDOW_OPEN.store(false, Ordering::Relaxed);
            }
        }
    }
}

/// Per-frame profiler hook. No-op unless the `profiling` feature is enabled.
#[cfg(feature = "profiling")]
pub fn frame(ctx: &egui::Context, active_tab: &'static str) {
    imp::frame(ctx, active_tab);
}

/// Per-frame profiler hook. No-op unless the `profiling` feature is enabled.
#[cfg(not(feature = "profiling"))]
#[inline(always)]
pub fn frame(_ctx: &egui::Context, _active_tab: &'static str) {}

/// Records this frame's work time; call at the very end of `update()`.
/// No-op unless the `profiling` feature is enabled.
#[cfg(feature = "profiling")]
pub fn end_frame(ctx: &egui::Context) {
    imp::end_frame(ctx);
}

/// Records this frame's work time; call at the very end of `update()`.
/// No-op unless the `profiling` feature is enabled.
#[cfg(not(feature = "profiling"))]
#[inline(always)]
pub fn end_frame(_ctx: &egui::Context) {}

/// Update sprite cache stats for periodic logging.
/// No-op unless the `profiling` feature is enabled.
#[cfg(feature = "profiling")]
pub fn update_cache_stats(glow: usize, dye: usize, char_dye: usize, bytes: usize) {
    imp::update_cache_stats(glow, dye, char_dye, bytes);
}

/// Update sprite cache stats for periodic logging.
/// No-op unless the `profiling` feature is enabled.
#[cfg(not(feature = "profiling"))]
#[inline(always)]
#[allow(dead_code)]
pub fn update_cache_stats(_glow: usize, _dye: usize, _char_dye: usize, _bytes: usize) {}

//! Bandwidth statistics tracking.
//!
//! Provides rolling window calculation for bytes-per-second metrics,
//! along with total byte counters for incoming and outgoing traffic.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Sample for rolling window calculation.
#[derive(Debug, Clone, Copy)]
struct Sample {
    /// When this sample was recorded
    timestamp: Instant,
    /// Bytes in this sample (incoming)
    bytes_in: u64,
    /// Bytes in this sample (outgoing)
    bytes_out: u64,
}

/// Bandwidth statistics with rolling window calculation.
#[derive(Debug)]
pub struct BandwidthStats {
    /// Rolling window of samples for rate calculation
    samples: VecDeque<Sample>,
    /// Window duration for rate calculation
    window_duration: Duration,
    /// Total bytes received (all time)
    total_bytes_in: u64,
    /// Total bytes sent (all time)
    total_bytes_out: u64,
    /// Total packets received (all time)
    total_packets_in: u64,
    /// Total packets sent (all time)
    total_packets_out: u64,
    /// Current bytes per second (incoming)
    bytes_per_sec_in: f64,
    /// Current bytes per second (outgoing)
    bytes_per_sec_out: f64,
    /// History of bytes-per-second values (for graphs)
    history_in: VecDeque<f64>,
    /// History of bytes-per-second values (for graphs)
    history_out: VecDeque<f64>,
    /// Maximum history entries to keep
    max_history: usize,
    /// Last time we calculated the rate
    last_calc: Option<Instant>,
}

impl Default for BandwidthStats {
    fn default() -> Self {
        Self::new()
    }
}

impl BandwidthStats {
    /// Default window duration (1 second).
    const DEFAULT_WINDOW: Duration = Duration::from_secs(1);
    /// Default history size (60 seconds).
    const DEFAULT_HISTORY_SIZE: usize = 60;

    /// Create a new bandwidth stats tracker.
    pub fn new() -> Self {
        Self {
            samples: VecDeque::new(),
            window_duration: Self::DEFAULT_WINDOW,
            total_bytes_in: 0,
            total_bytes_out: 0,
            total_packets_in: 0,
            total_packets_out: 0,
            bytes_per_sec_in: 0.0,
            bytes_per_sec_out: 0.0,
            history_in: VecDeque::with_capacity(Self::DEFAULT_HISTORY_SIZE),
            history_out: VecDeque::with_capacity(Self::DEFAULT_HISTORY_SIZE),
            max_history: Self::DEFAULT_HISTORY_SIZE,
            last_calc: None,
        }
    }

    /// Create with custom window duration and history size.
    pub fn with_config(window_duration: Duration, max_history: usize) -> Self {
        Self {
            samples: VecDeque::new(),
            window_duration,
            total_bytes_in: 0,
            total_bytes_out: 0,
            total_packets_in: 0,
            total_packets_out: 0,
            bytes_per_sec_in: 0.0,
            bytes_per_sec_out: 0.0,
            history_in: VecDeque::with_capacity(max_history),
            history_out: VecDeque::with_capacity(max_history),
            max_history,
            last_calc: None,
        }
    }

    /// Record incoming bytes.
    pub fn record_incoming(&mut self, bytes: u64) {
        self.total_bytes_in += bytes;
        self.total_packets_in += 1;
        self.add_sample(bytes, 0);
    }

    /// Record outgoing bytes.
    pub fn record_outgoing(&mut self, bytes: u64) {
        self.total_bytes_out += bytes;
        self.total_packets_out += 1;
        self.add_sample(0, bytes);
    }

    /// Record bytes with direction flag.
    pub fn record(&mut self, bytes: u64, incoming: bool) {
        if incoming {
            self.record_incoming(bytes);
        } else {
            self.record_outgoing(bytes);
        }
    }

    /// Add a sample to the rolling window.
    fn add_sample(&mut self, bytes_in: u64, bytes_out: u64) {
        let now = Instant::now();
        self.samples.push_back(Sample {
            timestamp: now,
            bytes_in,
            bytes_out,
        });
        self.prune_old_samples(now);
    }

    /// Remove samples outside the window.
    fn prune_old_samples(&mut self, now: Instant) {
        let cutoff = now - self.window_duration;
        while let Some(sample) = self.samples.front() {
            if sample.timestamp < cutoff {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    /// Update the bytes-per-second calculation.
    /// Call this periodically (e.g., once per second) to update history.
    pub fn update(&mut self) {
        let now = Instant::now();
        self.prune_old_samples(now);

        // Calculate bytes in the window
        let (bytes_in, bytes_out): (u64, u64) = self
            .samples
            .iter()
            .fold((0, 0), |acc, s| (acc.0 + s.bytes_in, acc.1 + s.bytes_out));

        // Calculate the actual duration covered by samples
        let duration_secs = self.window_duration.as_secs_f64();

        // Update rates
        self.bytes_per_sec_in = bytes_in as f64 / duration_secs;
        self.bytes_per_sec_out = bytes_out as f64 / duration_secs;

        // Add to history if enough time has passed (1 second intervals)
        let should_add_history = match self.last_calc {
            None => true,
            Some(last) => now.duration_since(last) >= Duration::from_secs(1),
        };

        if should_add_history {
            self.history_in.push_back(self.bytes_per_sec_in);
            self.history_out.push_back(self.bytes_per_sec_out);

            // Trim history to max size
            while self.history_in.len() > self.max_history {
                self.history_in.pop_front();
            }
            while self.history_out.len() > self.max_history {
                self.history_out.pop_front();
            }

            self.last_calc = Some(now);
        }
    }

    /// Get current incoming bytes per second.
    pub fn bytes_per_sec_in(&self) -> f64 {
        self.bytes_per_sec_in
    }

    /// Get current outgoing bytes per second.
    pub fn bytes_per_sec_out(&self) -> f64 {
        self.bytes_per_sec_out
    }

    /// Get total bytes per second (in + out).
    pub fn bytes_per_sec_total(&self) -> f64 {
        self.bytes_per_sec_in + self.bytes_per_sec_out
    }

    /// Get total incoming bytes.
    pub fn total_bytes_in(&self) -> u64 {
        self.total_bytes_in
    }

    /// Get total outgoing bytes.
    pub fn total_bytes_out(&self) -> u64 {
        self.total_bytes_out
    }

    /// Get total bytes (in + out).
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes_in + self.total_bytes_out
    }

    /// Get total incoming packets.
    pub fn total_packets_in(&self) -> u64 {
        self.total_packets_in
    }

    /// Get total outgoing packets.
    pub fn total_packets_out(&self) -> u64 {
        self.total_packets_out
    }

    /// Get total packets (in + out).
    pub fn total_packets(&self) -> u64 {
        self.total_packets_in + self.total_packets_out
    }

    /// Get incoming bandwidth history (for graphing).
    pub fn history_in(&self) -> &VecDeque<f64> {
        &self.history_in
    }

    /// Get outgoing bandwidth history (for graphing).
    pub fn history_out(&self) -> &VecDeque<f64> {
        &self.history_out
    }

    /// Reset all statistics.
    pub fn reset(&mut self) {
        self.samples.clear();
        self.total_bytes_in = 0;
        self.total_bytes_out = 0;
        self.total_packets_in = 0;
        self.total_packets_out = 0;
        self.bytes_per_sec_in = 0.0;
        self.bytes_per_sec_out = 0.0;
        self.history_in.clear();
        self.history_out.clear();
        self.last_calc = None;
    }

    /// Format bytes as human-readable string (B, KB, MB).
    pub fn format_bytes(bytes: u64) -> String {
        if bytes < 1024 {
            format!("{} B", bytes)
        } else if bytes < 1024 * 1024 {
            format!("{:.1} KB", bytes as f64 / 1024.0)
        } else if bytes < 1024 * 1024 * 1024 {
            format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
        } else {
            format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
        }
    }

    /// Format bytes-per-second as human-readable string.
    pub fn format_rate(bps: f64) -> String {
        if bps < 1024.0 {
            format!("{:.0} B/s", bps)
        } else if bps < 1024.0 * 1024.0 {
            format!("{:.1} KB/s", bps / 1024.0)
        } else {
            format!("{:.2} MB/s", bps / (1024.0 * 1024.0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let stats = BandwidthStats::new();
        assert_eq!(stats.total_bytes_in(), 0);
        assert_eq!(stats.total_bytes_out(), 0);
        assert_eq!(stats.total_packets_in(), 0);
        assert_eq!(stats.total_packets_out(), 0);
    }

    #[test]
    fn test_record_incoming() {
        let mut stats = BandwidthStats::new();
        stats.record_incoming(100);
        stats.record_incoming(200);
        assert_eq!(stats.total_bytes_in(), 300);
        assert_eq!(stats.total_packets_in(), 2);
        assert_eq!(stats.total_bytes_out(), 0);
    }

    #[test]
    fn test_record_outgoing() {
        let mut stats = BandwidthStats::new();
        stats.record_outgoing(50);
        stats.record_outgoing(150);
        assert_eq!(stats.total_bytes_out(), 200);
        assert_eq!(stats.total_packets_out(), 2);
        assert_eq!(stats.total_bytes_in(), 0);
    }

    #[test]
    fn test_record_with_direction() {
        let mut stats = BandwidthStats::new();
        stats.record(100, true); // incoming
        stats.record(50, false); // outgoing
        assert_eq!(stats.total_bytes_in(), 100);
        assert_eq!(stats.total_bytes_out(), 50);
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(BandwidthStats::format_bytes(500), "500 B");
        assert_eq!(BandwidthStats::format_bytes(1536), "1.5 KB");
        assert_eq!(BandwidthStats::format_bytes(1048576), "1.00 MB");
    }

    #[test]
    fn test_format_rate() {
        assert_eq!(BandwidthStats::format_rate(500.0), "500 B/s");
        assert_eq!(BandwidthStats::format_rate(1536.0), "1.5 KB/s");
        assert_eq!(BandwidthStats::format_rate(1048576.0), "1.00 MB/s");
    }

    #[test]
    fn test_reset() {
        let mut stats = BandwidthStats::new();
        stats.record_incoming(1000);
        stats.record_outgoing(500);
        assert!(stats.total_bytes() > 0);

        stats.reset();
        assert_eq!(stats.total_bytes(), 0);
        assert_eq!(stats.total_packets(), 0);
    }

    #[test]
    fn test_bytes_per_second_calculation() {
        let mut stats = BandwidthStats::with_config(Duration::from_millis(100), 10);

        // Record some data
        stats.record_incoming(1000);
        stats.update();

        // Rate should be approximately 10000 B/s (1000 bytes / 0.1 seconds)
        let rate = stats.bytes_per_sec_in();
        assert!(rate > 5000.0 && rate < 15000.0, "Rate was {}", rate);
    }
}

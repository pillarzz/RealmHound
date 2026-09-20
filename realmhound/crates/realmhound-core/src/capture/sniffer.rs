//! Packet sniffer for capturing network traffic.

use super::{error::CaptureError, interface::NetworkInterface, ROTMG_PORT};
use pcap::{Active, Capture};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Capacity of the bounded capture queue, in packets.
///
/// At a few thousand packets/sec (typical under a VPN with smaller MTUs) this
/// gives roughly 1-2 seconds of cushion to absorb brief GUI-frame stalls before
/// the queue starts dropping. Large enough to ride out hitches, small enough to
/// bound worst-case memory if the consumer fully stalls.
pub const DEFAULT_QUEUE_CAPACITY: usize = 4096;

/// Configuration for the packet sniffer.
#[derive(Debug, Clone)]
pub struct SnifferConfig {
    /// Read timeout in milliseconds
    pub timeout_ms: i32,
    /// Buffer size in bytes
    pub buffer_size: i32,
    /// Whether to capture in promiscuous mode
    pub promiscuous: bool,
    /// BPF filter expression (auto-generated if None)
    pub filter: Option<String>,
}

impl Default for SnifferConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 100,
            // Large kernel capture buffer to absorb bursts (e.g. under VPN,
            // where smaller MTUs mean higher packet rates) and avoid drops.
            buffer_size: 16 * 1024 * 1024, // 16MB buffer
            promiscuous: false,
            filter: None,
        }
    }
}

/// Packet sniffer that captures RotMG traffic.
pub struct Sniffer {
    capture: Capture<Active>,
    interface_name: String,
}

impl Sniffer {
    /// Create a new sniffer on the specified interface.
    ///
    /// # Errors
    ///
    /// Returns an error if the interface cannot be opened or the filter
    /// cannot be applied.
    pub fn new(interface: &NetworkInterface, config: SnifferConfig) -> Result<Self, CaptureError> {
        let capture = Capture::from_device(interface.name.as_str())
            .map_err(|e| CaptureError::InterfaceOpenFailed {
                name: interface.name.clone(),
                reason: e.to_string(),
            })?
            .timeout(config.timeout_ms)
            .buffer_size(config.buffer_size)
            .promisc(config.promiscuous)
            .open()
            .map_err(|e| CaptureError::InterfaceOpenFailed {
                name: interface.name.clone(),
                reason: e.to_string(),
            })?;

        let mut sniffer = Self {
            capture,
            interface_name: interface.name.clone(),
        };

        // Apply BPF filter for RotMG traffic
        let filter = config
            .filter
            .unwrap_or_else(|| format!("tcp port {ROTMG_PORT}"));
        sniffer.set_filter(&filter)?;

        Ok(sniffer)
    }

    /// Set a BPF filter on the capture.
    fn set_filter(&mut self, filter: &str) -> Result<(), CaptureError> {
        self.capture
            .filter(filter, true)
            .map_err(|e| CaptureError::FilterFailed(e.to_string()))
    }

    /// Get the next packet from the capture.
    ///
    /// Returns `None` if the read timed out with no packet available.
    pub fn next_packet(&mut self) -> Result<Option<RawPacket>, CaptureError> {
        match self.capture.next_packet() {
            Ok(packet) => {
                let ts = packet.header.ts;
                let timestamp =
                    chrono::DateTime::from_timestamp(ts.tv_sec as i64, (ts.tv_usec * 1000) as u32)
                        .unwrap_or_default();
                Ok(Some(RawPacket {
                    timestamp,
                    #[cfg(feature = "latency-diagnostics")]
                    enqueued_at: None,
                    data: packet.data.to_vec(),
                }))
            }
            Err(pcap::Error::TimeoutExpired) => Ok(None),
            Err(e) => Err(CaptureError::CaptureError(e.to_string())),
        }
    }

    /// Get the name of the interface being captured.
    pub fn interface_name(&self) -> &str {
        &self.interface_name
    }

    /// Get capture statistics (received / dropped / interface-dropped counts).
    ///
    /// `dropped` counts packets dropped because the kernel capture buffer was
    /// full - a sign the consumer can't keep up or the buffer is too small.
    pub fn stats(&mut self) -> Option<pcap::Stat> {
        self.capture.stats().ok()
    }
}

/// Log pcap drop statistics when new drops are observed.
///
/// Captures dropped by the kernel (npcap) buffer cause TCP stream gaps that
/// desync the cipher, so surfacing them helps diagnose flaky captures.
fn log_capture_stats(
    sniffer: &mut Sniffer,
    last_dropped: &mut u32,
    last_if_dropped: &mut u32,
    queue_dropped: u64,
    last_queue_dropped: &mut u64,
) {
    if let Some(stats) = sniffer.stats() {
        if stats.dropped > *last_dropped || stats.if_dropped > *last_if_dropped {
            tracing::warn!(
                "Capture drops detected: {} kernel-dropped, {} iface-dropped, {} received \
                 (npcap buffer may be too small or consumer too slow)",
                stats.dropped,
                stats.if_dropped,
                stats.received
            );
            *last_dropped = stats.dropped;
            *last_if_dropped = stats.if_dropped;
        }
    }
    if queue_dropped > *last_queue_dropped {
        tracing::warn!(
            "Capture queue overflow: {} packets dropped (oldest-first) because the GUI \
             consumer fell behind (queue capacity {})",
            queue_dropped,
            DEFAULT_QUEUE_CAPACITY
        );
        *last_queue_dropped = queue_dropped;
    }
}

/// A raw captured packet.
#[derive(Debug, Clone)]
pub struct RawPacket {
    /// Timestamp when the packet was captured
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Monotonic timestamp recorded immediately before capture-queue insertion.
    #[cfg(feature = "latency-diagnostics")]
    pub enqueued_at: Option<std::time::Instant>,
    /// Raw packet data including headers
    pub data: Vec<u8>,
}

impl RawPacket {
    /// Get the timestamp as a Duration since Unix epoch.
    pub fn timestamp_duration(&self) -> Duration {
        Duration::from_secs(self.timestamp.timestamp() as u64)
            + Duration::from_nanos(self.timestamp.timestamp_subsec_nanos() as u64)
    }
}

/// Shared state of the bounded, drop-oldest capture queue.
struct QueueInner {
    buf: VecDeque<RawPacket>,
    capacity: usize,
    /// Total packets dropped because the queue was full.
    dropped: u64,
}

struct PacketQueue {
    inner: Mutex<QueueInner>,
    /// Signalled whenever a packet is pushed or the last producer drops, so a
    /// blocked `PacketReceiver::recv_timeout` can wake to drain or observe close.
    not_empty: Condvar,
    /// Number of live `PacketSender`s. When this reaches zero the capture has
    /// fully ended, which the consumer uses to detect disconnect.
    producers: AtomicUsize,
    /// Cleared when the `PacketReceiver` is dropped, signalling producers to stop.
    consumer_alive: AtomicBool,
}

/// Sending half of the bounded capture queue.
///
/// Pushing never blocks: when the queue is full the *oldest* packet is evicted
/// and counted, so the capture thread can keep draining pcap (blocking it would
/// stall npcap and cause kernel-buffer drops). Cloning registers an additional
/// producer; the queue is considered closed once all senders have been dropped.
pub struct PacketSender {
    queue: Arc<PacketQueue>,
}

impl PacketSender {
    /// Push a packet, evicting the oldest if the queue is at capacity.
    ///
    /// Returns `Err(())` if the consumer has been dropped (capture should stop).
    pub fn send(&self, packet: RawPacket) -> Result<(), ()> {
        if !self.queue.consumer_alive.load(Ordering::Acquire) {
            return Err(());
        }
        #[cfg(feature = "latency-diagnostics")]
        let packet = {
            let mut packet = packet;
            packet.enqueued_at = Some(std::time::Instant::now());
            packet
        };
        let mut inner = self.queue.inner.lock().unwrap();
        if inner.buf.len() >= inner.capacity {
            inner.buf.pop_front();
            inner.dropped += 1;
        }
        inner.buf.push_back(packet);
        drop(inner);
        // Wake a consumer blocked in `recv_timeout`.
        self.queue.not_empty.notify_one();
        Ok(())
    }

    /// Total packets dropped so far due to a full queue.
    pub fn dropped(&self) -> u64 {
        self.queue.inner.lock().unwrap().dropped
    }
}

impl Clone for PacketSender {
    fn clone(&self) -> Self {
        self.queue.producers.fetch_add(1, Ordering::AcqRel);
        Self {
            queue: self.queue.clone(),
        }
    }
}

impl Drop for PacketSender {
    fn drop(&mut self) {
        // Decrement the producer count and notify under the queue lock so a
        // consumer parked in `recv_timeout` cannot miss the close transition.
        // The wait predicate reads `producers`, so the state change must be
        // synchronized with the same mutex the condvar uses: holding `inner`
        // here means the consumer either hasn't yet checked the predicate (and
        // will observe `producers == 0`) or is already parked (and will be woken).
        let _guard = self.queue.inner.lock().unwrap();
        self.queue.producers.fetch_sub(1, Ordering::AcqRel);
        self.queue.not_empty.notify_all();
    }
}

/// Result of draining the capture queue.
pub struct DrainResult {
    /// Packets drained (up to the requested maximum).
    pub packets: Vec<RawPacket>,
    /// True when all producers have exited and the queue is now empty,
    /// i.e. the capture has fully ended.
    pub closed: bool,
    /// Number of packets still buffered after this drain.
    pub queue_remaining: usize,
}

/// Receiving half of the bounded capture queue.
pub struct PacketReceiver {
    queue: Arc<PacketQueue>,
}

impl PacketReceiver {
    /// Drain up to `max` packets without blocking.
    ///
    /// The returned `closed` flag is computed while holding the queue lock and
    /// is only true once every producer has exited *and* the buffer is empty,
    /// so any buffered packets are always delivered before disconnect is reported.
    pub fn drain(&self, max: usize) -> DrainResult {
        let mut inner = self.queue.inner.lock().unwrap();
        let n = max.min(inner.buf.len());
        let packets: Vec<RawPacket> = inner.buf.drain(..n).collect();
        let closed = inner.buf.is_empty() && self.queue.producers.load(Ordering::Acquire) == 0;
        let queue_remaining = inner.buf.len();
        DrainResult {
            packets,
            closed,
            queue_remaining,
        }
    }

    /// Block until at least one packet is available, the capture closes, or
    /// `timeout` elapses, then drain up to `max` packets.
    ///
    /// Unlike [`drain`], this parks the calling thread on a `Condvar` instead of
    /// busy-polling, so a worker can sit idle until the capture thread pushes.
    /// The `closed` flag is only true once every producer has exited *and* the
    /// buffer is drained, so buffered packets are always delivered first.
    pub fn recv_timeout(&self, max: usize, timeout: Duration) -> DrainResult {
        let mut inner = self.queue.inner.lock().unwrap();
        if inner.buf.is_empty() && self.queue.producers.load(Ordering::Acquire) != 0 {
            let queue = &self.queue;
            let (guard, _) = queue
                .not_empty
                .wait_timeout_while(inner, timeout, |q| {
                    q.buf.is_empty() && queue.producers.load(Ordering::Acquire) != 0
                })
                .unwrap();
            inner = guard;
        }
        let n = max.min(inner.buf.len());
        let packets: Vec<RawPacket> = inner.buf.drain(..n).collect();
        let closed = inner.buf.is_empty() && self.queue.producers.load(Ordering::Acquire) == 0;
        let queue_remaining = inner.buf.len();
        DrainResult {
            packets,
            closed,
            queue_remaining,
        }
    }

    /// Total packets dropped by the queue (oldest evicted when full).
    pub fn dropped(&self) -> u64 {
        self.queue.inner.lock().unwrap().dropped
    }
}

impl Drop for PacketReceiver {
    fn drop(&mut self) {
        self.queue.consumer_alive.store(false, Ordering::Release);
    }
}

/// Create a bounded, drop-oldest capture queue with the given capacity.
///
/// The returned sender counts as the first producer; clone it for each capture
/// thread and drop the original once all threads have been spawned.
pub fn packet_channel(capacity: usize) -> (PacketSender, PacketReceiver) {
    let queue = Arc::new(PacketQueue {
        inner: Mutex::new(QueueInner {
            buf: VecDeque::with_capacity(capacity),
            capacity,
            dropped: 0,
        }),
        not_empty: Condvar::new(),
        producers: AtomicUsize::new(1),
        consumer_alive: AtomicBool::new(true),
    });
    (
        PacketSender {
            queue: queue.clone(),
        },
        PacketReceiver { queue },
    )
}

/// Handle for controlling a running capture.
#[derive(Clone)]
pub struct CaptureHandle {
    /// Flag to signal the capture loop to stop
    stop_flag: Arc<AtomicBool>,
}

impl CaptureHandle {
    /// Create a new capture handle.
    fn new() -> Self {
        Self {
            stop_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Stop the capture loop.
    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
    }

    /// Check if stop has been requested.
    pub fn is_stopped(&self) -> bool {
        self.stop_flag.load(Ordering::SeqCst)
    }
}

/// Start capturing packets in a background thread.
///
/// Returns a [`CaptureHandle`] to control the capture and a receiver for packets.
pub fn start_capture(
    interface: &NetworkInterface,
    config: SnifferConfig,
) -> Result<(CaptureHandle, PacketReceiver), CaptureError> {
    let mut sniffer = Sniffer::new(interface, config)?;
    let handle = CaptureHandle::new();
    let stop_flag = handle.stop_flag.clone();

    // Bounded, drop-oldest queue: the capture thread never blocks (so npcap
    // doesn't drop), but worst-case memory stays bounded if the GUI stalls.
    let (tx, rx) = packet_channel(DEFAULT_QUEUE_CAPACITY);

    // Spawn capture thread
    std::thread::spawn(move || {
        tracing::info!("Capture thread started");

        let mut last_dropped = 0u32;
        let mut last_if_dropped = 0u32;
        let mut last_queue_dropped = 0u64;
        let mut packet_count = 0u64;
        while !stop_flag.load(Ordering::SeqCst) {
            match sniffer.next_packet() {
                Ok(Some(packet)) => {
                    if tx.send(packet).is_err() {
                        // Receiver dropped, stop capturing
                        tracing::warn!("Packet receiver dropped, stopping capture");
                        break;
                    }
                    packet_count += 1;
                    if packet_count % 5000 == 0 {
                        log_capture_stats(
                            &mut sniffer,
                            &mut last_dropped,
                            &mut last_if_dropped,
                            tx.dropped(),
                            &mut last_queue_dropped,
                        );
                    }
                }
                Ok(None) => {
                    // Timeout, continue loop
                }
                Err(e) => {
                    tracing::error!("Capture error: {}", e);
                    break;
                }
            }
        }

        tracing::info!("Capture thread stopped");
    });

    Ok((handle, rx))
}

/// Start capturing packets with auto-detection of the correct interface.
///
/// Captures on all `interfaces` simultaneously, detects which one receives
/// RotMG traffic, then closes the rest. Returns the capture handle, the packet
/// receiver, and a oneshot receiver for the detected interface name.
pub fn start_capture_auto_detect(
    interfaces: &[NetworkInterface],
    config: SnifferConfig,
) -> Result<
    (
        CaptureHandle,
        PacketReceiver,
        tokio::sync::oneshot::Receiver<String>,
    ),
    CaptureError,
> {
    use std::sync::Mutex;

    if interfaces.is_empty() {
        return Err(CaptureError::NoInterfaces);
    }

    let handle = CaptureHandle::new();
    let stop_flag = handle.stop_flag.clone();

    // Bounded, drop-oldest queue (see start_capture). The template `tx` keeps the
    // producer count >= 1 while threads are being spawned; each thread gets a clone
    // and the template is dropped afterwards so `closed` only fires once every
    // surviving capture thread has exited.
    let (tx, rx) = packet_channel(DEFAULT_QUEUE_CAPACITY);

    // Channel to report which interface was detected
    let (detected_tx, detected_rx) = tokio::sync::oneshot::channel();
    let detected_tx = Arc::new(Mutex::new(Some(detected_tx)));

    // Flag to indicate when we've found the active interface
    let found_interface = Arc::new(AtomicBool::new(false));

    // Open sniffers on all viable interfaces
    let mut sniffer_handles = Vec::new();

    for interface in interfaces {
        // Skip interfaces without IPv4 addresses or that look inactive
        if !interface.is_likely_active() {
            tracing::debug!(
                "Skipping interface {} (no active IPv4)",
                interface.display_name()
            );
            continue;
        }

        match Sniffer::new(interface, config.clone()) {
            Ok(sniffer) => {
                let stop_flag = stop_flag.clone();
                let found = found_interface.clone();
                let tx = tx.clone();
                let detected_tx = detected_tx.clone();
                let iface_name = interface.display_name().to_string();

                let thread_handle = std::thread::spawn(move || {
                    capture_on_interface(sniffer, iface_name, stop_flag, found, tx, detected_tx);
                });

                sniffer_handles.push(thread_handle);
                tracing::info!("Started sniffer on {}", interface.display_name());
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to open {} for capture: {}",
                    interface.display_name(),
                    e
                );
            }
        }
    }

    // Drop the template sender so only the per-thread clones keep the queue open.
    drop(tx);

    if sniffer_handles.is_empty() {
        return Err(CaptureError::NoInterfaces);
    }

    tracing::info!(
        "Auto-detect started on {} interfaces",
        sniffer_handles.len()
    );

    Ok((handle, rx, detected_rx))
}

/// Capture loop for a single interface during auto-detection.
fn capture_on_interface(
    mut sniffer: Sniffer,
    interface_name: String,
    stop_flag: Arc<AtomicBool>,
    found_interface: Arc<AtomicBool>,
    tx: PacketSender,
    detected_tx: Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<String>>>>,
) {
    while !stop_flag.load(Ordering::SeqCst) {
        // If another interface was found, exit this thread
        if found_interface.load(Ordering::SeqCst) {
            tracing::debug!(
                "Interface {} stopping (another interface found)",
                interface_name
            );
            return;
        }

        match sniffer.next_packet() {
            Ok(Some(packet)) => {
                // We got a packet! This is the active interface.
                // Mark as found so other threads exit
                if !found_interface.swap(true, Ordering::SeqCst) {
                    // We're the first to find traffic
                    tracing::info!("Auto-detected RotMG traffic on {}", interface_name);

                    // Send the detected interface name
                    if let Ok(mut guard) = detected_tx.lock() {
                        if let Some(tx) = guard.take() {
                            let _ = tx.send(interface_name.clone());
                        }
                    }
                }

                // Send the packet
                if tx.send(packet).is_err() {
                    tracing::warn!("Packet receiver dropped");
                    return;
                }

                // Continue capturing on this interface only
                capture_remaining(sniffer, stop_flag, tx);
                return;
            }
            Ok(None) => {
                // Timeout, continue
            }
            Err(e) => {
                tracing::debug!("Interface {} error: {}", interface_name, e);
                return;
            }
        }
    }
}

/// Continue capturing on the detected interface after auto-detection.
fn capture_remaining(mut sniffer: Sniffer, stop_flag: Arc<AtomicBool>, tx: PacketSender) {
    let mut last_dropped = 0u32;
    let mut last_if_dropped = 0u32;
    let mut last_queue_dropped = 0u64;
    let mut packet_count = 0u64;
    while !stop_flag.load(Ordering::SeqCst) {
        match sniffer.next_packet() {
            Ok(Some(packet)) => {
                if tx.send(packet).is_err() {
                    return;
                }
                packet_count += 1;
                if packet_count % 5000 == 0 {
                    log_capture_stats(
                        &mut sniffer,
                        &mut last_dropped,
                        &mut last_if_dropped,
                        tx.dropped(),
                        &mut last_queue_dropped,
                    );
                }
            }
            Ok(None) => {}
            Err(e) => {
                tracing::error!("Capture error: {}", e);
                return;
            }
        }
    }
    tracing::info!("Capture stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(byte: u8) -> RawPacket {
        RawPacket {
            timestamp: chrono::Utc::now(),
            #[cfg(feature = "latency-diagnostics")]
            enqueued_at: None,
            data: vec![byte],
        }
    }

    #[test]
    fn queue_delivers_in_order() {
        let (tx, rx) = packet_channel(8);
        tx.send(raw(1)).unwrap();
        tx.send(raw(2)).unwrap();
        let result = rx.drain(10);
        assert_eq!(result.packets.len(), 2);
        assert_eq!(result.packets[0].data, vec![1]);
        assert_eq!(result.packets[1].data, vec![2]);
        assert!(!result.closed); // sender still alive
        assert_eq!(tx.dropped(), 0);
    }

    #[test]
    fn queue_drops_oldest_on_overflow() {
        let (tx, rx) = packet_channel(2);
        tx.send(raw(1)).unwrap();
        tx.send(raw(2)).unwrap();
        tx.send(raw(3)).unwrap(); // evicts oldest (1)
        assert_eq!(tx.dropped(), 1);
        let result = rx.drain(10);
        assert_eq!(result.packets.len(), 2);
        assert_eq!(result.packets[0].data, vec![2]);
        assert_eq!(result.packets[1].data, vec![3]);
    }

    #[test]
    fn drain_respects_max() {
        let (tx, rx) = packet_channel(8);
        for i in 0..5 {
            tx.send(raw(i)).unwrap();
        }
        let first = rx.drain(3);
        assert_eq!(first.packets.len(), 3);
        assert_eq!(first.queue_remaining, 2);
        assert_eq!(rx.drain(10).packets.len(), 2);
    }

    #[cfg(feature = "latency-diagnostics")]
    #[test]
    fn send_stamps_enqueue_time_at_queue_boundary() {
        let (tx, rx) = packet_channel(1);
        let before = std::time::Instant::now();
        tx.send(raw(1)).unwrap();
        let packet = rx.drain(1).packets.pop().unwrap();
        assert!(packet.enqueued_at.is_some_and(|at| at >= before));
    }

    #[test]
    fn closed_only_after_all_producers_drop_and_drained() {
        let (tx, rx) = packet_channel(8);
        let tx2 = tx.clone();
        tx.send(raw(1)).unwrap();
        drop(tx);
        // One producer remains, so not closed even though buffer will empty.
        let r = rx.drain(10);
        assert_eq!(r.packets.len(), 1);
        assert!(!r.closed);
        drop(tx2);
        // All producers gone and buffer empty => closed.
        let r = rx.drain(10);
        assert!(r.packets.is_empty());
        assert!(r.closed);
    }

    #[test]
    fn buffered_packets_delivered_before_closed() {
        let (tx, rx) = packet_channel(8);
        tx.send(raw(42)).unwrap();
        drop(tx);
        // Last producer gone but a packet is still buffered: deliver it AND
        // report closed in the same drain.
        let r = rx.drain(10);
        assert_eq!(r.packets.len(), 1);
        assert_eq!(r.packets[0].data, vec![42]);
        assert!(r.closed);
    }

    #[test]
    fn send_fails_after_receiver_dropped() {
        let (tx, rx) = packet_channel(8);
        drop(rx);
        assert!(tx.send(raw(1)).is_err());
    }

    #[test]
    fn recv_timeout_returns_immediately_when_data_buffered() {
        let (tx, rx) = packet_channel(8);
        tx.send(raw(7)).unwrap();
        let start = std::time::Instant::now();
        let r = rx.recv_timeout(10, Duration::from_secs(5));
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(r.packets.len(), 1);
        assert_eq!(r.packets[0].data, vec![7]);
        assert!(!r.closed);
    }

    #[test]
    fn recv_timeout_times_out_when_empty() {
        let (_tx, rx) = packet_channel(8);
        let start = std::time::Instant::now();
        let r = rx.recv_timeout(10, Duration::from_millis(50));
        assert!(start.elapsed() >= Duration::from_millis(40));
        assert!(r.packets.is_empty());
        assert!(!r.closed); // producer still alive
    }

    #[test]
    fn recv_timeout_wakes_on_send() {
        let (tx, rx) = packet_channel(8);
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            tx.send(raw(9)).unwrap();
            tx // keep producer alive
        });
        let r = rx.recv_timeout(10, Duration::from_secs(5));
        assert_eq!(r.packets.len(), 1);
        assert_eq!(r.packets[0].data, vec![9]);
        let _tx = handle.join().unwrap();
    }

    #[test]
    fn recv_timeout_reports_closed_when_all_producers_drop() {
        let (tx, rx) = packet_channel(8);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            drop(tx);
        });
        let r = rx.recv_timeout(10, Duration::from_secs(5));
        assert!(r.packets.is_empty());
        assert!(r.closed);
    }
}

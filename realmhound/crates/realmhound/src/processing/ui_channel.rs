//! A bounded, drop-oldest worker -> UI update channel.
//!
//! The worker emits a stream of *transient append* [`UiUpdate`]s (chat, loot,
//! boss calls, encounters). These accumulate legitimately while the window is
//! minimized and must NOT be coalesced or routinely dropped — they are the
//! append history the UI replays on refocus. The coalesced, latest-value path is
//! the `ModelSnapshot`, not this channel.
//!
//! Standard library channels offer either unbounded growth (`channel`) or
//! drop-*newest* back-pressure (`sync_channel` + `try_send`). Neither matches the
//! desired safety policy, so this is a small purpose-built queue:
//!
//! - Generously bounded (see [`DEFAULT_BOUND`]) so a long minimize never trips it
//!   in practice.
//! - On overflow: drop the OLDEST update (preserving the most recent history),
//!   increment a dropped counter, and warn (throttled) — this is an anomaly, not
//!   a normal condition.
//! - The dropped counter is surfaced to the UI (status bar) and the logs.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::contract::UiUpdate;

/// Default capacity of the worker -> UI update queue.
///
/// Sized generously: at a heavy ~hundreds-of-updates/sec a multi-minute minimize
/// stays well under this, so overflow only ever signals a genuine UI stall.
pub const DEFAULT_BOUND: usize = 65_536;

/// Minimum gap between overflow warnings, so a sustained overflow doesn't spam
/// the log.
const WARN_THROTTLE: Duration = Duration::from_secs(5);

struct Shared {
    queue: Mutex<VecDeque<UiUpdate>>,
    /// Total number of updates dropped due to overflow (monotonic).
    dropped: AtomicU64,
    /// Cleared when the receiver is dropped (UI gone).
    receiver_alive: AtomicBool,
}

/// Worker-side sender. Never blocks; drops the oldest update on overflow.
pub struct UiSender {
    shared: Arc<Shared>,
    bound: usize,
    /// Last time an overflow warning was logged (throttle).
    last_warn: Mutex<Option<Instant>>,
}

/// UI-side receiver. Drained (non-blocking) each frame.
pub struct UiReceiver {
    shared: Arc<Shared>,
}

/// Error returned by [`UiSender::send`] when the UI receiver has been dropped.
pub struct Disconnected;

/// Create a bounded drop-oldest update channel with the given capacity.
pub fn channel(bound: usize) -> (UiSender, UiReceiver) {
    let shared = Arc::new(Shared {
        queue: Mutex::new(VecDeque::with_capacity(bound.min(1024))),
        dropped: AtomicU64::new(0),
        receiver_alive: AtomicBool::new(true),
    });
    (
        UiSender {
            shared: shared.clone(),
            bound,
            last_warn: Mutex::new(None),
        },
        UiReceiver { shared },
    )
}

impl UiSender {
    /// Enqueue an update. On overflow the oldest queued update is dropped and the
    /// dropped counter is bumped. Returns `Err(Disconnected)` only if the UI
    /// receiver has been dropped.
    pub fn send(&self, update: UiUpdate) -> Result<(), Disconnected> {
        if !self.shared.receiver_alive.load(Ordering::Acquire) {
            return Err(Disconnected);
        }
        #[cfg(feature = "latency-diagnostics")]
        let update = {
            let mut update = update;
            update.enqueued_at = Some(Instant::now());
            update
        };
        let mut overflowed = false;
        {
            let mut q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            if q.len() >= self.bound {
                q.pop_front();
                overflowed = true;
            }
            q.push_back(update);
        }
        if overflowed {
            let total = self.shared.dropped.fetch_add(1, Ordering::Relaxed) + 1;
            self.maybe_warn(total);
        }
        Ok(())
    }

    fn maybe_warn(&self, total: u64) {
        let now = Instant::now();
        let mut last = self.last_warn.lock().unwrap_or_else(|e| e.into_inner());
        let should = match *last {
            Some(prev) => now.duration_since(prev) >= WARN_THROTTLE,
            None => true,
        };
        if should {
            *last = Some(now);
            tracing::warn!(
                "[WORKER] UI update channel overflow (cap {}); dropped {} oldest update(s) total. \
                 The UI may be stalled.",
                self.bound,
                total
            );
        }
    }
}

impl UiReceiver {
    /// Pop the next queued update, if any. Non-blocking.
    pub fn try_recv(&self) -> Option<UiUpdate> {
        self.shared
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
    }

    /// Total updates dropped so far due to overflow (surfaced in the status bar).
    pub fn dropped(&self) -> u64 {
        self.shared.dropped.load(Ordering::Relaxed)
    }

    /// Number of updates currently waiting for the UI.
    #[cfg(feature = "latency-diagnostics")]
    pub fn len(&self) -> usize {
        self.shared
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }
}

impl Drop for UiReceiver {
    fn drop(&mut self) {
        self.shared.receiver_alive.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing::contract::{UiPayload, UiUpdate};

    fn update(seq: u64) -> UiUpdate {
        UiUpdate {
            seq,
            #[cfg(feature = "latency-diagnostics")]
            enqueued_at: None,
            payload: UiPayload::LootBatchAdded,
        }
    }

    #[test]
    fn delivers_in_order_under_capacity() {
        let (tx, rx) = channel(8);
        for i in 0..5 {
            tx.send(update(i)).ok();
        }
        for i in 0..5 {
            assert_eq!(rx.try_recv().unwrap().seq, i);
        }
        assert!(rx.try_recv().is_none());
        assert_eq!(rx.dropped(), 0);
    }

    #[test]
    fn drops_oldest_on_overflow() {
        let (tx, rx) = channel(3);
        // Send 5 into a cap-3 queue: the two oldest (0,1) are dropped.
        for i in 0..5 {
            tx.send(update(i)).ok();
        }
        assert_eq!(rx.dropped(), 2);
        let seqs: Vec<u64> = std::iter::from_fn(|| rx.try_recv().map(|u| u.seq)).collect();
        assert_eq!(seqs, vec![2, 3, 4]);
    }

    #[test]
    fn send_errors_after_receiver_dropped() {
        let (tx, rx) = channel(4);
        drop(rx);
        assert!(tx.send(update(0)).is_err());
    }

    #[cfg(feature = "latency-diagnostics")]
    #[test]
    fn send_stamps_enqueue_time() {
        let (tx, rx) = channel(1);
        let before = Instant::now();
        assert!(tx.send(update(0)).is_ok());
        assert!(rx
            .try_recv()
            .unwrap()
            .enqueued_at
            .is_some_and(|at| at >= before));
    }
}

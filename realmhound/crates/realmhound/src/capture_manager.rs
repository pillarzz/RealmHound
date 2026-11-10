/// Manages the packet capture lifecycle.
///
/// Owns the network interfaces, capture handle, packet channel,
/// and interface auto-detection. Extracted from
/// `RealmHoundApp` to isolate capture concerns.
use realmhound_core::capture::{
    start_capture_auto_detect, CaptureHandle, NetworkInterface, PacketReceiver, SnifferConfig,
};

/// Manages the packet capture lifecycle: start, stop, drain packets.
pub struct CaptureManager {
    /// Available network interfaces (loaded once at startup).
    interfaces: Result<Vec<NetworkInterface>, String>,
    /// Detected interface name (after auto-detection picks the active one).
    detected_interface: Option<String>,
    /// Whether capture is currently running.
    capturing: bool,
    /// Handle to stop the capture thread.
    capture_handle: Option<CaptureHandle>,
    /// Bounded, drop-oldest receiver for captured raw packets.
    packet_rx: Option<PacketReceiver>,
    /// Receiver for the auto-detected interface name.
    detected_interface_rx: Option<tokio::sync::oneshot::Receiver<String>>,
}

#[cfg(target_os = "linux")]
impl Drop for CaptureManager {
    fn drop(&mut self) {
        if let Some(handle) = &self.capture_handle {
            handle.stop();
        }
    }
}

#[allow(dead_code)]
impl CaptureManager {
    /// Create a new `CaptureManager`.
    ///
    /// `interfaces` is the result of enumerating network interfaces at startup.
    pub fn new(interfaces: Result<Vec<NetworkInterface>, String>) -> Self {
        Self {
            interfaces,
            detected_interface: None,
            capturing: false,
            capture_handle: None,
            packet_rx: None,
            detected_interface_rx: None,
        }
    }

    /// Whether capture is currently running.
    pub fn is_capturing(&self) -> bool {
        self.capturing
    }

    /// The detected interface name, if auto-detection has completed.
    pub fn detected_interface(&self) -> Option<&str> {
        self.detected_interface.as_deref()
    }

    /// Start packet capture with auto-detection of the active interface.
    ///
    /// Returns `Ok(())` on success, `Err(msg)` if startup fails.
    /// This clears any previous capture state.
    pub fn start(&mut self) -> Result<(), String> {
        let interfaces = match &self.interfaces {
            Ok(list) if !list.is_empty() => list.clone(),
            Ok(_) => return Err("No network interfaces found".into()),
            Err(e) => return Err(format!("Interface error: {}", e)),
        };

        match start_capture_auto_detect(&interfaces, SnifferConfig::default()) {
            Ok((handle, rx, detected_rx)) => {
                self.capture_handle = Some(handle);
                self.packet_rx = Some(rx);
                self.detected_interface_rx = Some(detected_rx);
                self.capturing = true;
                self.detected_interface = None;

                let count = interfaces.len();
                tracing::info!("Auto-detect capture started on {} interfaces", count);
                Ok(())
            }
            Err(e) => {
                let msg = format!("Failed to start capture: {}", e);
                tracing::error!("{}", msg);
                Err(msg)
            }
        }
    }

    /// Poll for the auto-detected interface name.
    ///
    /// Call this each frame. Once the interface is detected, subsequent
    /// calls are no-ops and `detected_interface()` returns the name.
    pub fn poll_detected_interface(&mut self) {
        if let Some(rx) = &mut self.detected_interface_rx {
            match rx.try_recv() {
                Ok(name) => {
                    tracing::info!("Auto-detected active interface: {}", name);
                    self.detected_interface = Some(name);
                    self.detected_interface_rx = None;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.detected_interface_rx = None;
                }
            }
        }
    }

    /// Take ownership of the packet receiver so it can be handed to the worker
    /// thread, which drains it directly. Returns `None` if capture is not running.
    pub fn take_receiver(&mut self) -> Option<PacketReceiver> {
        self.packet_rx.take()
    }

    /// Stop and clear the capture after the source closed, so a retry starts
    /// from a clean state.
    pub fn mark_stopped(&mut self) {
        if let Some(handle) = &self.capture_handle {
            handle.stop();
        }
        self.capturing = false;
        self.capture_handle = None;
        self.packet_rx = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use realmhound_core::capture::packet_channel;

    #[test]
    fn new_is_not_capturing() {
        let mgr = CaptureManager::new(Ok(vec![]));
        assert!(!mgr.is_capturing());
        assert!(mgr.detected_interface().is_none());
    }

    #[test]
    fn new_with_error_preserves_error() {
        let mgr = CaptureManager::new(Err("test error".into()));
        assert!(!mgr.is_capturing());
        // start() should propagate the error
        let mut mgr = mgr;
        let result = mgr.start();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("test error"));
    }

    #[test]
    fn start_fails_with_empty_interfaces() {
        let mut mgr = CaptureManager::new(Ok(vec![]));
        let result = mgr.start();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No network interfaces"));
        assert!(!mgr.is_capturing());
    }

    #[test]
    fn poll_detected_interface_with_no_rx() {
        let mut mgr = CaptureManager::new(Ok(vec![]));
        // Should not panic
        mgr.poll_detected_interface();
        assert!(mgr.detected_interface().is_none());
    }

    #[test]
    fn poll_detected_interface_receives_name() {
        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        let mut mgr = CaptureManager::new(Ok(vec![]));
        mgr.detected_interface_rx = Some(rx);

        tx.send("eth0".to_string()).unwrap();

        mgr.poll_detected_interface();
        assert_eq!(mgr.detected_interface(), Some("eth0"));
    }

    #[test]
    fn mark_stopped_clears_state() {
        let (_tx, rx) = packet_channel(64);
        let mut mgr = CaptureManager::new(Ok(vec![]));
        mgr.packet_rx = Some(rx);
        mgr.capturing = true;

        mgr.mark_stopped();

        assert!(!mgr.is_capturing());
        assert!(mgr.packet_rx.is_none());
        assert!(mgr.capture_handle.is_none());
    }
}

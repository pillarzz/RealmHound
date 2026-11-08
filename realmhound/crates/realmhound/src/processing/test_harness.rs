//! Reusable test harness for exercising the worker without a UI.
//!
//! The worker owns capture, processing and persistence; this harness starts it
//! against a temporary selected profile and deliberately never drains the
//! worker -> UI queue, so tests observe the no-UI path directly.

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use eframe::egui;
use realmhound_core::account::{
    AccountContext, InMemoryCredentialStore, StartupResolution, StartupResolver,
};
use realmhound_core::capture::PacketReceiver;
use realmhound_core::storage::StorageRoot;
use realmhound_core::vault::AccountData;
use realmhound_core::Settings;

use crate::audio::AudioSender;
use crate::processing::contract::{ControlMsg, ShutdownReport};
use crate::processing::health::{CaptureState, HealthSnapshot};
use crate::processing::processor::PacketProcessor;
use crate::processing::worker::{spawn, WorkerHandle};

/// A staged worker over a temporary profile, with no egui view attached.
pub(crate) struct NoUiWorker {
    /// Worker control handle (also exposes the UI receiver and health counters).
    pub(crate) handle: WorkerHandle,
    /// Kept alive so the worker's profile lock and storage root outlive the test.
    #[allow(dead_code)]
    account: Arc<AccountContext>,
    /// Temporary storage root backing the profile.
    #[allow(dead_code)]
    root: tempfile::TempDir,
}

impl NoUiWorker {
    /// Spawn a staged worker over a seeded empty profile.
    pub(crate) fn start() -> Self {
        Self::start_with(|_| {})
    }

    /// Spawn a staged worker with a pending chat update.
    pub(crate) fn start_with_chat(message: crate::panels::chat::ChatMessage) -> Self {
        Self::start_with(|processor| processor.queue_chat_for_test(message))
    }

    fn start_with(seed: impl FnOnce(&mut PacketProcessor)) -> Self {
        let root = tempfile::tempdir().unwrap();
        let storage = StorageRoot::from_path(root.path()).unwrap();
        std::fs::write(
            storage.path().join("settings.json"),
            br#"{"account":{"account_id":"ACCT-STAGE","account_name":"Hero"}}"#,
        )
        .unwrap();
        std::fs::write(
            storage.path().join("account_data.json"),
            serde_json::to_vec(&AccountData::new()).unwrap(),
        )
        .unwrap();

        let credentials = Arc::new(InMemoryCredentialStore::new());
        let account = match StartupResolver::new(storage, None, credentials).resolve() {
            StartupResolution::Selected(selected) => Arc::new(selected.into_context()),
            _ => panic!("expected a selected startup for the seeded known layout"),
        };

        let persistence = account.persistence();
        let mut processor = PacketProcessor::new_selected(
            Arc::new(RwLock::new(Settings::default())),
            AccountData::new(),
            persistence.account_data_repository(),
            persistence.loot_database().to_path_buf(),
            persistence.combat_database().to_path_buf(),
            Some(account.account_id().to_string()),
            None,
        )
        .expect("writers open for the selected profile");
        seed(&mut processor);

        let handle = spawn(
            processor,
            egui::Context::default(),
            AudioSender::disconnected(),
            account.clone(),
        )
        .expect("worker thread spawns");

        Self {
            handle,
            account,
            root,
        }
    }

    /// Start and activate the worker so it processes and persists.
    pub(crate) fn start_active() -> Self {
        let harness = Self::start();
        harness.handle.activate();
        harness
    }

    /// Attach a capture source to the worker.
    pub(crate) fn start_capture(&self, receiver: PacketReceiver) {
        self.handle.send_control(ControlMsg::StartCapture(receiver));
    }

    /// Current always-on health, read without draining the UI queue.
    pub(crate) fn health(&self) -> HealthSnapshot {
        self.handle
            .health
            .snapshot(self.handle.ui_rx.len(), self.handle.ui_rx.dropped())
    }

    /// Wait for the worker to publish the expected capture state.
    pub(crate) fn wait_for_capture(&self, expected: CaptureState) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.health().capture == expected {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// Flush and join the worker.
    pub(crate) fn shutdown(&mut self) -> ShutdownReport {
        self.handle.shutdown(Duration::from_secs(5))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::chat::{ChatMessage, ChatType};
    use chrono::{Local, TimeZone};
    use realmhound_core::capture::packet_channel;

    #[test]
    fn no_ui_worker_reports_capture_lifecycle() {
        let mut harness = NoUiWorker::start_active();
        assert_eq!(harness.health().capture, CaptureState::Stopped);

        let (sender, receiver) = packet_channel(8);
        harness.start_capture(receiver);
        assert!(
            harness.wait_for_capture(CaptureState::Active),
            "worker did not attach the capture source"
        );

        // Closing the source is observable without any UI frame being drained.
        drop(sender);
        assert!(
            harness.wait_for_capture(CaptureState::Closed),
            "worker did not report the closed capture source"
        );
        let health = harness.health();
        assert_eq!(health.drain_frames, 0, "no UI frames may be implied");
        assert_eq!(health.drained_updates, 0);

        // Retry: attaching a fresh source must return the worker to Active.
        let (retry_sender, retry_receiver) = packet_channel(8);
        harness.start_capture(retry_receiver);
        assert!(
            harness.wait_for_capture(CaptureState::Active),
            "worker did not attach the retried capture source"
        );
        drop(retry_sender);

        assert!(
            harness.shutdown().is_success(),
            "clean shutdown must still succeed"
        );
    }

    #[test]
    fn chat_is_persisted_without_a_ui_receiver() {
        let message = ChatMessage {
            timestamp: Local.with_ymd_and_hms(2026, 2, 3, 0, 0, 5).unwrap(),
            sender: "Pillar".to_string(),
            text: "hello".to_string(),
            chat_type: ChatType::Normal,
            recipient: None,
            stars: 0,
            is_supporter: false,
            boss_id: None,
            is_outgoing: false,
        };
        let mut harness = NoUiWorker::start_with_chat(message);
        harness.handle.activate();

        let path = harness
            .account
            .persistence()
            .chat_logs()
            .join("chat_2026-02-03.log");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let contents =
            std::fs::read_to_string(&path).expect("chat line written without a UI drain");
        assert!(contents.contains("00:00:05 [PUBLIC] Pillar: hello"));
        assert_eq!(harness.health().drain_frames, 0, "no UI frames observed");
        assert!(harness.shutdown().is_success());
    }
}

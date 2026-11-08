//! Packet-processing model extraction.
//!
//! `PacketProcessor` owns the full packet model and runs the entire
//! `process_packets` + `dispatch_event` pipeline, emitting derived
//! [`UiUpdate`]s and a coalesced [`ViewState`] snapshot. The UI applies the
//! updates and reads the snapshot. Session 1 drives this synchronously; the
//! contract types are shaped for a Session 2 worker-thread move.

pub mod chat_log;
pub mod contract;
pub mod health;
pub mod mission_tracker;
pub mod processor;
pub mod realm_discovery;
pub mod ui_channel;
pub mod worker;

#[cfg(test)]
pub(crate) mod test_harness;

pub use contract::{
    AccountOperationScope, AudioCommand, ControlMsg, UiPayload, UiUpdate, ViewState,
};
pub use health::HealthMonitor;
pub use mission_tracker::{
    MissionCategory, MissionEntryView, MissionView, ObjectiveKind, ObjectiveView, RewardGroup,
    RewardView, WornReqView,
};
pub use processor::PacketProcessor;
#[allow(unused_imports)]
pub use processor::ProcessorInitError;
pub use worker::{spawn as spawn_worker, WorkerHandle};

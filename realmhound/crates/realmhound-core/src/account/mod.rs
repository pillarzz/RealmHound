//! Versioned account registry and isolated profile paths.

mod credentials;
mod identity;
mod lock;
mod paths;
mod persistence;
mod registry;
mod startup;

pub mod migration;

#[cfg(windows)]
pub use credentials::WindowsCredentialStore;
pub use credentials::{
    credential_target, CredentialError, CredentialStore, InMemoryCredentialStore, SavedCredential,
};
pub use identity::{AccountId, AccountIdError, AccountKey, AccountKeyError};
pub use lock::{ProfileLock, ProfileLockError};
pub use paths::{AccountPaths, ProfileManifest, ProfileSchemas};
pub use persistence::AccountPersistencePaths;
pub use registry::{
    shorten_duplicate_keys, AccountError, AccountRegistry, AccountRegistryEntry,
    AccountRegistryStore, MigrationKnownBinding, PendingTransition, ProfileHealthIssue,
    ProfileHealthKind, ReconciledRegistry, ReconciliationReport, RegistryMode, TransitionReason,
};
pub use startup::{
    AccountContext, DiscoveryStartup, RecoveryStartup, SelectedStartup, StartupError,
    StartupOutcome, StartupResolution, StartupResolver,
};

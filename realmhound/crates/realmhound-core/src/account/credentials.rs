//! Account access-token credential storage.
//!
//! Tokens are stored outside the profile and registry, keyed by a deterministic
//! target derived from the opaque [`AccountKey`]. The registry only ever holds a
//! credential *target reference*, never the token itself. Reads, overwrites, and
//! deletes go through the testable [`CredentialStore`] trait; the production
//! Windows implementation is backed by Windows Credential Manager.
//!
//! Errors are typed and redacted: no variant carries token contents, and
//! [`SavedCredential`] redacts its token in `Debug`. When the store is
//! unavailable, callers treat it as "no credential" and wait for the next
//! verified HELLO; there is no plaintext fallback.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::AccountKey;

/// A stored access token and the time it was captured.
///
/// The token is redacted in `Debug` output so it can never be logged.
#[derive(Clone, Serialize, Deserialize)]
pub struct SavedCredential {
    token: String,
    #[serde(default)]
    captured_at: Option<DateTime<Utc>>,
}

impl SavedCredential {
    /// Create a credential from a captured token and its capture time.
    pub fn new(token: impl Into<String>, captured_at: Option<DateTime<Utc>>) -> Self {
        Self {
            token: token.into(),
            captured_at,
        }
    }

    /// Borrow the token contents. Callers must never log the returned value.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Return when the token was captured, if known.
    pub fn captured_at(&self) -> Option<DateTime<Utc>> {
        self.captured_at
    }
}

impl fmt::Debug for SavedCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SavedCredential")
            .field("token", &"<redacted>")
            .field("captured_at", &self.captured_at)
            .finish()
    }
}

impl PartialEq for SavedCredential {
    fn eq(&self, other: &Self) -> bool {
        self.token == other.token && self.captured_at == other.captured_at
    }
}

/// Errors returned by a [`CredentialStore`]. No variant carries token contents.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CredentialError {
    /// The credential backend is not available on this platform or session.
    /// Callers treat this as "no credential" and wait for the next verified
    /// HELLO, never falling back to plaintext.
    #[error("credential store is unavailable")]
    Unavailable,
    /// The target name was empty or otherwise rejected by the backend.
    #[error("credential target is invalid")]
    InvalidTarget,
    /// The stored credential payload could not be decoded.
    #[error("stored credential payload is malformed")]
    MalformedPayload,
    /// The backend reported a low-level failure, identified only by its code.
    #[error("credential store operation failed (code {code})")]
    Backend {
        /// Platform error code (never contains token contents).
        code: u32,
    },
}

/// Derive the deterministic credential target for an account key:
/// `RealmHound/account/<opaque AccountKey>`.
pub fn credential_target(account_key: AccountKey) -> String {
    format!("RealmHound/account/{account_key}")
}

/// Testable storage interface for account access tokens.
///
/// `read` returns `Ok(None)` when no credential exists for the target. `write`
/// overwrites any existing credential. `delete` is idempotent: removing a
/// missing credential is `Ok(())`.
pub trait CredentialStore: Send + Sync {
    /// Read the credential stored under `target`, or `None` when none exists.
    fn read(&self, target: &str) -> Result<Option<SavedCredential>, CredentialError>;

    /// Write (overwrite) the credential stored under `target`.
    fn write(&self, target: &str, credential: &SavedCredential) -> Result<(), CredentialError>;

    /// Delete the credential stored under `target`. Missing is not an error.
    fn delete(&self, target: &str) -> Result<(), CredentialError>;
}

/// In-memory credential store used by tests and as a non-Windows fallback.
///
/// Never touches the real Credential Manager, so tests stay hermetic.
#[derive(Debug, Default)]
pub struct InMemoryCredentialStore {
    entries: std::sync::Mutex<std::collections::HashMap<String, SavedCredential>>,
}

impl InMemoryCredentialStore {
    /// Create an empty in-memory store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl CredentialStore for InMemoryCredentialStore {
    fn read(&self, target: &str) -> Result<Option<SavedCredential>, CredentialError> {
        if target.is_empty() {
            return Err(CredentialError::InvalidTarget);
        }
        let entries = self
            .entries
            .lock()
            .map_err(|_| CredentialError::Unavailable)?;
        Ok(entries.get(target).cloned())
    }

    fn write(&self, target: &str, credential: &SavedCredential) -> Result<(), CredentialError> {
        if target.is_empty() {
            return Err(CredentialError::InvalidTarget);
        }
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| CredentialError::Unavailable)?;
        entries.insert(target.to_string(), credential.clone());
        Ok(())
    }

    fn delete(&self, target: &str) -> Result<(), CredentialError> {
        if target.is_empty() {
            return Err(CredentialError::InvalidTarget);
        }
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| CredentialError::Unavailable)?;
        entries.remove(target);
        Ok(())
    }
}

#[cfg(windows)]
pub use windows_impl::WindowsCredentialStore;

#[cfg(windows)]
mod windows_impl {
    use super::{CredentialError, CredentialStore, SavedCredential};

    use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
        CRED_TYPE_GENERIC,
    };

    /// Windows Credential Manager implementation of [`CredentialStore`].
    ///
    /// The token payload is stored as JSON in the credential blob under a
    /// generic credential whose target name is the deterministic account target.
    #[derive(Debug, Default)]
    pub struct WindowsCredentialStore;

    impl WindowsCredentialStore {
        /// Create a Windows Credential Manager store.
        pub fn new() -> Self {
            Self
        }
    }

    fn to_wide_nul(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    impl CredentialStore for WindowsCredentialStore {
        fn read(&self, target: &str) -> Result<Option<SavedCredential>, CredentialError> {
            if target.is_empty() {
                return Err(CredentialError::InvalidTarget);
            }
            let target_w = to_wide_nul(target);
            let mut credential_ptr: *mut CREDENTIALW = std::ptr::null_mut();
            // SAFETY: `target_w` is a valid nul-terminated wide string, and
            // `credential_ptr` receives an owned allocation freed via `CredFree`.
            let ok =
                unsafe { CredReadW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential_ptr) };
            if ok == 0 {
                // SAFETY: querying the thread's last error code is always sound.
                let code = unsafe { GetLastError() };
                if code == ERROR_NOT_FOUND {
                    return Ok(None);
                }
                return Err(CredentialError::Backend { code });
            }
            if credential_ptr.is_null() {
                return Err(CredentialError::MalformedPayload);
            }

            // SAFETY: `CredReadW` succeeded, so `credential_ptr` points to a
            // valid `CREDENTIALW` whose blob spans `CredentialBlobSize` bytes.
            let result = unsafe {
                let credential = &*credential_ptr;
                if credential.CredentialBlob.is_null() || credential.CredentialBlobSize == 0 {
                    Err(CredentialError::MalformedPayload)
                } else {
                    let bytes = std::slice::from_raw_parts(
                        credential.CredentialBlob,
                        credential.CredentialBlobSize as usize,
                    );
                    serde_json::from_slice::<SavedCredential>(bytes)
                        .map_err(|_| CredentialError::MalformedPayload)
                }
            };
            // SAFETY: `credential_ptr` was allocated by `CredReadW`.
            unsafe { CredFree(credential_ptr as *mut _) };
            result.map(Some)
        }

        fn write(&self, target: &str, credential: &SavedCredential) -> Result<(), CredentialError> {
            if target.is_empty() {
                return Err(CredentialError::InvalidTarget);
            }
            let payload =
                serde_json::to_vec(credential).map_err(|_| CredentialError::MalformedPayload)?;
            let mut target_w = to_wide_nul(target);
            let mut user_w = to_wide_nul("RealmHound");

            let mut cred: CREDENTIALW = unsafe { std::mem::zeroed() };
            cred.Type = CRED_TYPE_GENERIC;
            cred.TargetName = target_w.as_mut_ptr();
            cred.CredentialBlobSize = payload.len() as u32;
            cred.CredentialBlob = payload.as_ptr() as *mut u8;
            cred.Persist = CRED_PERSIST_LOCAL_MACHINE;
            cred.UserName = user_w.as_mut_ptr();

            // SAFETY: all pointers outlive the call and reference valid buffers.
            let ok = unsafe { CredWriteW(&cred, 0) };
            if ok == 0 {
                // SAFETY: querying the thread's last error code is always sound.
                let code = unsafe { GetLastError() };
                return Err(CredentialError::Backend { code });
            }
            Ok(())
        }

        fn delete(&self, target: &str) -> Result<(), CredentialError> {
            if target.is_empty() {
                return Err(CredentialError::InvalidTarget);
            }
            let target_w = to_wide_nul(target);
            // SAFETY: `target_w` is a valid nul-terminated wide string.
            let ok = unsafe { CredDeleteW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0) };
            if ok == 0 {
                // SAFETY: querying the thread's last error code is always sound.
                let code = unsafe { GetLastError() };
                if code == ERROR_NOT_FOUND {
                    return Ok(());
                }
                return Err(CredentialError::Backend { code });
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_is_deterministic_and_namespaced() {
        let key = AccountKey::generate();
        let target = credential_target(key);
        assert_eq!(target, format!("RealmHound/account/{key}"));
        assert_eq!(target, credential_target(key));
    }

    #[test]
    fn in_memory_read_overwrite_delete_round_trip() {
        let store = InMemoryCredentialStore::new();
        let target = credential_target(AccountKey::generate());
        assert_eq!(store.read(&target).unwrap(), None);

        let first = SavedCredential::new("token-one", Some(Utc::now()));
        store.write(&target, &first).unwrap();
        assert_eq!(store.read(&target).unwrap().unwrap().token(), "token-one");

        // Overwrite replaces the prior credential.
        let second = SavedCredential::new("token-two", None);
        store.write(&target, &second).unwrap();
        assert_eq!(store.read(&target).unwrap().unwrap().token(), "token-two");

        store.delete(&target).unwrap();
        assert_eq!(store.read(&target).unwrap(), None);
        // Deleting a missing credential is not an error.
        store.delete(&target).unwrap();
    }

    #[test]
    fn empty_target_is_rejected() {
        let store = InMemoryCredentialStore::new();
        let credential = SavedCredential::new("x", None);
        assert_eq!(store.read(""), Err(CredentialError::InvalidTarget));
        assert_eq!(
            store.write("", &credential),
            Err(CredentialError::InvalidTarget)
        );
        assert_eq!(store.delete(""), Err(CredentialError::InvalidTarget));
    }

    #[test]
    fn debug_redacts_token_contents() {
        let credential = SavedCredential::new("super-secret-token", None);
        let rendered = format!("{credential:?}");
        assert!(!rendered.contains("super-secret-token"));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn errors_never_carry_token_contents() {
        // Typed, redacted errors: their Display never includes a token.
        for error in [
            CredentialError::Unavailable,
            CredentialError::InvalidTarget,
            CredentialError::MalformedPayload,
            CredentialError::Backend { code: 1168 },
        ] {
            let rendered = error.to_string();
            assert!(!rendered.contains("token"));
        }
    }
}

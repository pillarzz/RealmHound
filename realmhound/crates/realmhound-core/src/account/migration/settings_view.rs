//! Strict, explicit-path reader for the legacy account section of `settings.json`.
//!
//! Unlike [`crate::settings::Settings::load`], this never resolves a real
//! `%LOCALAPPDATA%`, never relocates a corrupt file, and never defaults or
//! renames a corrupt document. It reads exactly the account identity fields the
//! known-account migration consumes and fails closed on malformed input.

use std::path::Path;

use serde::Deserialize;

use super::MigrationError;
use crate::account::{AccountId, AccountIdError};

/// The account identity strictly read from a legacy `settings.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyAccountSettings {
    /// Saved server account identity, present only for a known account.
    pub account_id: Option<AccountId>,
    /// Saved display name, if any.
    pub account_name: Option<String>,
    /// Last-client-seen Unix seconds (0 when unknown).
    pub last_client_seen_unix: i64,
    /// Last-client-launch Unix seconds (0 when unknown).
    pub last_client_launch_unix: i64,
}

impl LegacyAccountSettings {
    /// Whether settings hold a saved account ID, marking a known account.
    pub fn is_known(&self) -> bool {
        self.account_id.is_some()
    }
}

#[derive(Debug, Deserialize)]
struct SettingsView {
    #[serde(default)]
    account: AccountSectionView,
}

#[derive(Debug, Default, Deserialize)]
struct AccountSectionView {
    #[serde(default)]
    account_name: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    last_client_seen_unix: i64,
    #[serde(default)]
    last_client_launch_unix: i64,
}

/// Strictly read the account identity from an explicit `settings.json` path.
///
/// Returns `Ok(None)` only when the file is absent. A present but corrupt
/// document is an error, never a silent default. A present, empty account ID is
/// rejected rather than renamed.
pub fn read_legacy_account_settings(
    path: &Path,
) -> Result<Option<LegacyAccountSettings>, MigrationError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(MigrationError::SettingsRead {
                path: path.to_path_buf(),
                source: error,
            })
        }
    };

    let view: SettingsView =
        serde_json::from_slice(&bytes).map_err(|source| MigrationError::SettingsParse {
            path: path.to_path_buf(),
            source,
        })?;

    let account_id = match view.account.account_id {
        Some(raw) => Some(AccountId::new(&raw).map_err(|error: AccountIdError| {
            MigrationError::SettingsAccountId {
                path: path.to_path_buf(),
                source: error,
            }
        })?),
        None => None,
    };

    Ok(Some(LegacyAccountSettings {
        account_id,
        account_name: view.account.account_name,
        last_client_seen_unix: view.account.last_client_seen_unix,
        last_client_launch_unix: view.account.last_client_launch_unix,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, body: &str) -> std::path::PathBuf {
        let path = dir.join("settings.json");
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn missing_file_returns_none() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        assert_eq!(read_legacy_account_settings(&path).unwrap(), None);
    }

    #[test]
    fn reads_known_account_identity_and_timestamps() {
        let temp = tempfile::tempdir().unwrap();
        let path = write(
            temp.path(),
            r#"{"account":{"account_name":"Hero","account_id":"ABC123","last_client_seen_unix":11,"last_client_launch_unix":22}}"#,
        );

        let settings = read_legacy_account_settings(&path).unwrap().unwrap();

        assert!(settings.is_known());
        assert_eq!(settings.account_id.unwrap().as_str(), "ABC123");
        assert_eq!(settings.account_name.as_deref(), Some("Hero"));
        assert_eq!(settings.last_client_seen_unix, 11);
        assert_eq!(settings.last_client_launch_unix, 22);
    }

    #[test]
    fn absent_account_id_is_unknown_not_defaulted() {
        let temp = tempfile::tempdir().unwrap();
        let path = write(temp.path(), r#"{"account":{"account_name":"Hero"}}"#);

        let settings = read_legacy_account_settings(&path).unwrap().unwrap();

        assert!(!settings.is_known());
        assert_eq!(settings.account_id, None);
    }

    #[test]
    fn corrupt_document_is_error_and_not_relocated() {
        let temp = tempfile::tempdir().unwrap();
        let path = write(temp.path(), "{ not json");

        let error = read_legacy_account_settings(&path).unwrap_err();

        assert!(matches!(error, MigrationError::SettingsParse { .. }));
        assert!(path.exists(), "corrupt settings must not be relocated");
    }

    #[test]
    fn empty_account_id_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let path = write(temp.path(), r#"{"account":{"account_id":"   "}}"#);

        let error = read_legacy_account_settings(&path).unwrap_err();

        assert!(matches!(error, MigrationError::SettingsAccountId { .. }));
    }
}

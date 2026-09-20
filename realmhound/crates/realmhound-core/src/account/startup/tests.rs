//! Startup resolver tests. Every root is an explicit tempfile path and every
//! credential store is in-memory; nothing resolves a real local data directory
//! or a real credential backend. The `test_sources_never_resolve_real_roots`
//! guard enforces that for this file.

use std::sync::Arc;

use chrono::Utc;
use tempfile::TempDir;

use super::{StartupError, StartupResolution, StartupResolver};
use crate::account::migration::journal::{load_journal, MigrationPhase};
use crate::account::migration::{FlatLayoutMigration, MigrationOutcome};
use crate::account::{
    AccountId, AccountKey, AccountPaths, AccountRegistryStore, InMemoryCredentialStore,
    ProfileLock, RegistryMode, SavedCredential,
};
use crate::storage::StorageRoot;
use crate::vault::AccountData;

fn root() -> (TempDir, StorageRoot) {
    let temp = tempfile::tempdir().unwrap();
    let root = StorageRoot::from_path(temp.path()).unwrap();
    (temp, root)
}

fn creds() -> Arc<InMemoryCredentialStore> {
    Arc::new(InMemoryCredentialStore::new())
}

fn resolver(root: &StorageRoot, credentials: Arc<InMemoryCredentialStore>) -> StartupResolver {
    StartupResolver::new(root.clone(), None, credentials)
}

fn write(root: &StorageRoot, relative: &str, bytes: &[u8]) {
    let path = root.path().join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn seed_loot_db(root: &StorageRoot, relative: &str) {
    let path = root.path().join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    // Leave rows resident in the WAL (no checkpoint) so migration must exercise
    // the WAL-held-row path.
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE loot_drops (id INTEGER PRIMARY KEY);
         CREATE TABLE loot_items (id INTEGER PRIMARY KEY);
         INSERT INTO loot_drops (id) VALUES (1);",
    )
    .unwrap();
}

fn seed_combat_db(root: &StorageRoot, relative: &str) {
    let path = root.path().join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE fights (id INTEGER PRIMARY KEY);
         CREATE TABLE fight_participants (fight_id INTEGER, object_id INTEGER);
         INSERT INTO fights (id) VALUES (1);",
    )
    .unwrap();
}

/// Seed a complete known flat layout including a plaintext token to import.
fn seed_known_flat_layout(root: &StorageRoot, account_id: &str) {
    write(
        root,
        "settings.json",
        format!(
            r#"{{"account":{{"account_id":"{account_id}","account_name":"Hero","last_client_seen_unix":100,"last_client_launch_unix":200}}}}"#
        )
        .as_bytes(),
    );
    write(
        root,
        "account_data.json",
        &serde_json::to_vec(&AccountData::new()).unwrap(),
    );
    write(root, "quests.json", br#"{"quests":[]}"#);
    write(
        root,
        "char_list.xml",
        br#"<Chars nextCharId="2"><Char id="1"></Char></Chars>"#,
    );
    write(root, "access_token.txt", b"secret-token-value");
    seed_loot_db(root, "loot_history.db");
    seed_combat_db(root, "combat_history.db");
}

/// Drive a real known migration to completion without holding a profile lock,
/// returning the migrated account key.
fn complete_known_migration(
    root: &StorageRoot,
    credentials: Arc<InMemoryCredentialStore>,
    account_id: &str,
) -> AccountKey {
    seed_known_flat_layout(root, account_id);
    let outcome = FlatLayoutMigration::new(root.clone(), None, credentials)
        .run_at(Utc::now())
        .unwrap();
    match outcome {
        MigrationOutcome::ReadyKnown { account_key, .. } => account_key,
        other => panic!("expected ReadyKnown, got {other:?}"),
    }
}

fn manifest_path(root: &StorageRoot, key: AccountKey) -> std::path::PathBuf {
    root.path()
        .join("accounts")
        .join(key.to_string())
        .join("profile.json")
}

fn launches(root: &StorageRoot) -> u32 {
    load_journal(root)
        .unwrap()
        .map(|journal| journal.backup.validated_launches)
        .unwrap_or(0)
}

#[test]
fn first_startup_completes_known_flat_data_through_resolver() {
    let (_temp, root) = root();
    let credentials = creds();
    seed_known_flat_layout(&root, "ACCT-1");

    let resolution = resolver(&root, credentials.clone()).resolve();
    let StartupResolution::Selected(selected) = resolution else {
        panic!("expected Selected startup");
    };
    let context = selected.context();

    // Verified identity comes from the migrated profile manifest.
    assert_eq!(context.account_id().as_str(), "ACCT-1");
    // The credential is available through the credential store, and the plaintext
    // token is gone from the flat layout.
    assert_eq!(
        context.read_credential().unwrap().token(),
        "secret-token-value"
    );
    assert!(!root.path().join("access_token.txt").exists());

    // Every account path is under the profile root, never the flat layout.
    let profile_root = root
        .path()
        .join("accounts")
        .join(context.account_key().to_string());
    assert!(context
        .persistence()
        .account_data()
        .starts_with(&profile_root));
    assert!(context
        .persistence()
        .loot_database()
        .starts_with(&profile_root));

    // The resolver never records a launch; recording is explicit and once.
    assert_eq!(launches(&root), 0);
    context.record_validated_launch().unwrap();
    assert_eq!(launches(&root), 1);
}

#[test]
fn selected_profile_startup_opens_the_committed_profile() {
    let (_temp, root) = root();
    let credentials = creds();
    let key = complete_known_migration(&root, credentials.clone(), "ACCT-1");

    let resolution = resolver(&root, credentials).resolve();
    let StartupResolution::Selected(selected) = resolution else {
        panic!("expected Selected startup");
    };
    assert_eq!(selected.context().account_key(), key);
    // The strict snapshot load succeeds for a legitimate profile.
    selected.context().load_account_data_strict().unwrap();
}

#[test]
fn completed_migration_startup_resolves_registry() {
    let (_temp, root) = root();
    let credentials = creds();
    let key = complete_known_migration(&root, credentials.clone(), "ACCT-1");
    // The migration is already Complete before this startup runs.
    assert_eq!(
        load_journal(&root).unwrap().unwrap().phase,
        MigrationPhase::Complete
    );

    let resolution = resolver(&root, credentials).resolve();
    let StartupResolution::Selected(selected) = resolution else {
        panic!("expected Selected startup");
    };
    assert_eq!(selected.context().account_key(), key);
}

#[test]
fn interrupted_migration_resumes_to_selected_startup() {
    let (_temp, root) = root();
    let credentials = creds();
    seed_known_flat_layout(&root, "ACCT-1");

    // Interrupt the first pass before the registry commit.
    let interrupted = FlatLayoutMigration::new(root.clone(), None, credentials.clone());
    interrupted.arm_failpoint("copies-validated");
    assert!(interrupted.run_at(Utc::now()).is_err());

    // The resolver constructs a fresh migration that resumes to completion.
    let resolution = resolver(&root, credentials).resolve();
    let StartupResolution::Selected(selected) = resolution else {
        panic!("expected Selected startup after resume");
    };
    assert_eq!(selected.context().account_id().as_str(), "ACCT-1");
    assert_eq!(
        load_journal(&root).unwrap().unwrap().phase,
        MigrationPhase::Complete
    );
}

#[test]
fn unknown_account_resolves_to_discovery_without_account_resources() {
    let (_temp, root) = root();
    let credentials = creds();
    // No account id in settings: an unknown migration quarantines nothing here
    // and awaits attribution.

    let resolution = resolver(&root, credentials).resolve();
    assert!(matches!(resolution, StartupResolution::Discovery(_)));

    // Discovery opened no account profile or database.
    let accounts = root.read_directory("accounts").unwrap();
    assert!(
        accounts.is_empty(),
        "discovery must open no account profile"
    );
    assert!(!root.path().join("accounts").join("databases").exists());
}

#[test]
fn no_selected_profile_resolves_to_discovery() {
    let (_temp, root) = root();
    let credentials = creds();
    complete_known_migration(&root, credentials.clone(), "ACCT-1");
    // The user deselected the profile; the registry is in discovery mode.
    AccountRegistryStore::new(root.clone()).deselect().unwrap();

    let resolution = resolver(&root, credentials).resolve();
    assert!(matches!(resolution, StartupResolution::Discovery(_)));
}

#[test]
fn discovery_cancel_restores_previous_selection() {
    let (_temp, root) = root();
    let credentials = creds();
    let key = complete_known_migration(&root, credentials.clone(), "ACCT-1");
    AccountRegistryStore::new(root.clone()).deselect().unwrap();

    let resolution = resolver(&root, credentials).resolve();
    let StartupResolution::Discovery(discovery) = resolution else {
        panic!("expected Discovery startup");
    };
    assert_eq!(discovery.previous_account_key(), Some(key));

    let restored = discovery.cancel_discovery(Utc::now()).unwrap();
    assert_eq!(restored.mode(), RegistryMode::Selected);
    assert_eq!(restored.selected_account_key(), Some(key));
}

#[test]
fn missing_profile_resolves_to_recovery() {
    let (_temp, root) = root();
    let credentials = creds();
    let key = complete_known_migration(&root, credentials.clone(), "ACCT-1");
    // Remove the selected profile directory entirely.
    std::fs::remove_dir_all(root.path().join("accounts").join(key.to_string())).unwrap();

    let resolution = resolver(&root, credentials).resolve();
    let StartupResolution::Recovery(recovery) = resolution else {
        panic!("expected Recovery startup");
    };
    assert!(matches!(
        recovery.error(),
        StartupError::MissingProfile | StartupError::ProfileUnhealthy(_)
    ));
    // A failed startup never records a launch.
    assert_eq!(launches(&root), 0);
}

#[test]
fn corrupt_manifest_resolves_to_recovery() {
    let (_temp, root) = root();
    let credentials = creds();
    let key = complete_known_migration(&root, credentials.clone(), "ACCT-1");
    std::fs::write(manifest_path(&root, key), b"{ not json").unwrap();
    // A rolling backup could still recover the manifest; remove it too.
    let backup = manifest_path(&root, key).with_extension("json.bak");
    let _ = std::fs::remove_file(backup);
    let sibling_bak = root
        .path()
        .join("accounts")
        .join(key.to_string())
        .join("profile.json.bak");
    let _ = std::fs::remove_file(sibling_bak);

    let resolution = resolver(&root, credentials).resolve();
    assert!(matches!(resolution, StartupResolution::Recovery(_)));
    assert_eq!(launches(&root), 0);
}

#[test]
fn future_manifest_version_resolves_to_recovery() {
    let (_temp, root) = root();
    let credentials = creds();
    let key = complete_known_migration(&root, credentials.clone(), "ACCT-1");
    // Bump the manifest version beyond what this build supports.
    let path = manifest_path(&root, key);
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["version"] = serde_json::json!(999);
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();

    let resolution = resolver(&root, credentials).resolve();
    let StartupResolution::Recovery(recovery) = resolution else {
        panic!("expected Recovery startup");
    };
    assert!(matches!(
        recovery.error(),
        StartupError::UnsupportedVersion | StartupError::ProfileUnhealthy(_)
    ));
    assert_eq!(launches(&root), 0);
}

#[test]
fn registry_manifest_identity_mismatch_resolves_to_recovery() {
    let (_temp, root) = root();
    let credentials = creds();
    let key = complete_known_migration(&root, credentials.clone(), "ACCT-1");
    // Rebind the manifest identity while the registry keeps the original.
    let path = manifest_path(&root, key);
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["account_id"] = serde_json::json!("OTHER-ACCOUNT");
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();

    let resolution = resolver(&root, credentials).resolve();
    let StartupResolution::Recovery(recovery) = resolution else {
        panic!("expected Recovery startup");
    };
    assert!(matches!(
        recovery.error(),
        StartupError::ManifestMismatch | StartupError::ProfileUnhealthy(_)
    ));
    assert_eq!(launches(&root), 0);
}

#[test]
fn lock_contention_resolves_to_recovery_without_recording_launch() {
    let (_temp, root) = root();
    let credentials = creds();
    let key = complete_known_migration(&root, credentials.clone(), "ACCT-1");
    // Another instance already holds the profile lock.
    let _held = ProfileLock::acquire(&root, key).unwrap();

    let resolution = resolver(&root, credentials).resolve();
    let StartupResolution::Recovery(recovery) = resolution else {
        panic!("expected Recovery startup");
    };
    assert_eq!(recovery.error(), &StartupError::Locked);
    assert_eq!(launches(&root), 0);
}

#[test]
fn successful_startup_records_launch_exactly_once() {
    let (_temp, root) = root();
    let credentials = creds();
    complete_known_migration(&root, credentials.clone(), "ACCT-1");

    let resolution = resolver(&root, credentials).resolve();
    let StartupResolution::Selected(selected) = resolution else {
        panic!("expected Selected startup");
    };
    assert_eq!(launches(&root), 0);
    selected.context().record_validated_launch().unwrap();
    assert_eq!(launches(&root), 1);
}

#[test]
fn two_profiles_with_equal_generation_never_cross_paths() {
    let (_temp, root) = root();
    let credentials = creds();
    let selected_key = complete_known_migration(&root, credentials.clone(), "ACCT-A");
    // A second, unselected profile with an equal internal generation counter.
    let other = AccountRegistryStore::new(root.clone())
        .register_verified(AccountId::new("ACCT-B").unwrap(), Some("Other"), Utc::now())
        .unwrap();

    let mut selected_data = AccountData::new();
    selected_data.generation = 7;
    selected_data.max_num_chars = 3;
    let mut other_data = AccountData::new();
    other_data.generation = 7;
    other_data.max_num_chars = 9;

    let selected_paths = AccountPaths::new(root.clone(), selected_key);
    let other_paths = AccountPaths::new(root.clone(), other.key());
    std::fs::write(
        selected_paths.account_data().unwrap(),
        serde_json::to_vec(&selected_data).unwrap(),
    )
    .unwrap();
    std::fs::write(
        other_paths.account_data().unwrap(),
        serde_json::to_vec(&other_data).unwrap(),
    )
    .unwrap();

    let resolution = resolver(&root, credentials).resolve();
    let StartupResolution::Selected(selected) = resolution else {
        panic!("expected Selected startup");
    };
    assert_eq!(selected.context().account_key(), selected_key);
    // Despite equal generations, the resolver binds strictly to the selected
    // profile's data, never the other profile's.
    let loaded = selected.context().load_account_data_strict().unwrap();
    assert_eq!(loaded.max_num_chars, 3);
}

#[test]
fn selected_context_paths_all_derive_from_one_profile_root() {
    let (_temp, root) = root();
    let credentials = creds();
    complete_known_migration(&root, credentials.clone(), "ACCT-1");

    let resolution = resolver(&root, credentials).resolve();
    let StartupResolution::Selected(selected) = resolution else {
        panic!("expected Selected startup");
    };
    let context = selected.context();
    let profile_root = context.paths().root().unwrap();
    for path in [
        context.persistence().account_data(),
        context.persistence().char_list_cache(),
        context.persistence().quests(),
        context.persistence().loot_database(),
        context.persistence().combat_database(),
        context.persistence().realmshark_import(),
        context.persistence().chat_logs(),
    ] {
        assert!(
            path.starts_with(&profile_root),
            "path {path:?} escaped the profile root {profile_root:?}"
        );
    }
    // Profile mode never exposes the legacy flat token path.
    assert!(context.persistence().legacy_access_token().is_none());
}

#[test]
fn corrupt_snapshot_fails_repeatedly_rather_than_becoming_empty() {
    let (_temp, root) = root();
    let credentials = creds();
    let key = complete_known_migration(&root, credentials.clone(), "ACCT-1");
    let account_data = AccountPaths::new(root.clone(), key).account_data().unwrap();
    std::fs::write(&account_data, b"{ not json").unwrap();
    let _ = std::fs::remove_file(account_data.with_extension("json.bak"));
    let sibling = root
        .path()
        .join("accounts")
        .join(key.to_string())
        .join("account_data.json.bak");
    let _ = std::fs::remove_file(sibling);

    let resolution = resolver(&root, credentials).resolve();
    let StartupResolution::Selected(selected) = resolution else {
        panic!("expected Selected startup");
    };
    let context = selected.context();
    // The snapshot is corrupt: strict load errors and never becomes empty.
    assert!(context.load_account_data_strict().is_err());
    // Bytes are untouched, so a retry keeps failing.
    assert_eq!(std::fs::read(&account_data).unwrap(), b"{ not json");
    assert!(context.load_account_data_strict().is_err());
}

#[test]
fn credential_store_errors_never_block_startup() {
    let (_temp, root) = root();
    // A credential store whose reads always fail must not block selected startup.
    #[derive(Debug, Default)]
    struct FailingReadStore;
    impl crate::account::CredentialStore for FailingReadStore {
        fn read(
            &self,
            _target: &str,
        ) -> Result<Option<SavedCredential>, crate::account::CredentialError> {
            Err(crate::account::CredentialError::Unavailable)
        }
        fn write(
            &self,
            _target: &str,
            _credential: &SavedCredential,
        ) -> Result<(), crate::account::CredentialError> {
            Ok(())
        }
        fn delete(&self, _target: &str) -> Result<(), crate::account::CredentialError> {
            Ok(())
        }
    }

    let credentials = creds();
    let key = complete_known_migration(&root, credentials, "ACCT-1");
    // Swap in a failing store for the resolver.
    let failing: Arc<dyn crate::account::CredentialStore> = Arc::new(FailingReadStore);
    let resolution = StartupResolver::new(root.clone(), None, failing).resolve();
    let StartupResolution::Selected(selected) = resolution else {
        panic!("expected Selected startup despite credential failure");
    };
    assert_eq!(selected.context().account_key(), key);
    // The failing read is a non-blocking "no token".
    assert!(selected.context().read_credential().is_none());
}

/// Seed a partial known flat layout: a known account identity and snapshot, but
/// no history databases, quests, char list, imports, or token.
fn seed_partial_known_layout(root: &StorageRoot, account_id: &str) {
    write(
        root,
        "settings.json",
        format!(r#"{{"account":{{"account_id":"{account_id}","account_name":"Hero"}}}}"#)
            .as_bytes(),
    );
    write(
        root,
        "account_data.json",
        &serde_json::to_vec(&AccountData::new()).unwrap(),
    );
}

#[test]
fn partial_known_flat_layout_resolves_through_resolver() {
    let (_temp, root) = root();
    let credentials = creds();
    seed_partial_known_layout(&root, "ACCT-PART");

    let resolution = resolver(&root, credentials).resolve();
    let StartupResolution::Selected(selected) = resolution else {
        panic!("expected Selected startup for a partial known layout");
    };
    let context = selected.context();
    assert_eq!(context.account_id().as_str(), "ACCT-PART");
    // The snapshot loads even though no history databases were migrated.
    context.load_account_data_strict().unwrap();
    // Every account path is under the profile root, never the flat layout.
    let profile_root = root
        .path()
        .join("accounts")
        .join(context.account_key().to_string());
    assert!(context
        .persistence()
        .account_data()
        .starts_with(&profile_root));
}

#[test]
fn known_migration_creates_a_backup_of_relocated_flat_sources() {
    let (_temp, root) = root();
    let credentials = creds();
    seed_known_flat_layout(&root, "ACCT-1");

    let resolution = resolver(&root, credentials).resolve();
    assert!(matches!(resolution, StartupResolution::Selected(_)));

    // A completed known migration relocates the flat originals into a backup.
    let backups = root.path().join("migration-backups");
    assert!(backups.is_dir(), "migration must create a backup directory");
    let has_backup = std::fs::read_dir(&backups)
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| entry.path().is_dir());
    assert!(has_backup, "the backup directory must hold a relocated set");
    // The flat originals are gone from the root (moved into the backup).
    assert!(!root.path().join("account_data.json").exists());
}

#[test]
fn unknown_flat_data_is_quarantined_and_resolves_to_discovery() {
    let (_temp, root) = root();
    let credentials = creds();
    // Flat data with no saved account id: an unknown migration quarantines the
    // data and resolves to discovery, opening no account profile.
    write(&root, "settings.json", br#"{"account":{}}"#);
    write(
        &root,
        "account_data.json",
        &serde_json::to_vec(&AccountData::new()).unwrap(),
    );
    seed_loot_db(&root, "loot_history.db");

    let resolution = resolver(&root, credentials).resolve();
    assert!(matches!(resolution, StartupResolution::Discovery(_)));

    // The unknown data is quarantined, not opened as a selected profile.
    let quarantine = root.path().join("quarantine").join("legacy-unassigned");
    assert!(quarantine.join("account_data.json").exists());
    assert!(quarantine.join("loot_history.db").exists());
    let accounts = root.read_directory("accounts").unwrap();
    assert!(
        accounts.is_empty(),
        "discovery must open no account profile"
    );
}

#[test]
fn resolve_startup_returns_settings_from_the_explicit_root() {
    let (_temp, root) = root();
    let credentials = creds();
    // A known layout with a custom global setting to prove settings come from
    // this exact root, migrated forward, alongside the resolution.
    seed_known_flat_layout(&root, "ACCT-1");
    std::fs::write(
        root.path().join("settings.json"),
        br#"{"version":1,"account":{"account_id":"ACCT-1","account_name":"Hero"},"sound":{"volume":0.25}}"#,
    )
    .unwrap();

    let outcome = resolver(&root, credentials).resolve_startup();
    assert!(matches!(outcome.resolution, StartupResolution::Selected(_)));
    // Loaded from the explicit root and migrated to the current version.
    assert_eq!(outcome.settings.sound.volume, 0.25);
    assert!(outcome.settings.version >= 7);
}

#[test]
fn corrupt_settings_resolve_to_recovery_and_stay_in_place() {
    let (_temp, root) = root();
    let credentials = creds();
    let settings_path = root.path().join("settings.json");
    let malformed = b"{ not settings json";
    std::fs::write(&settings_path, malformed).unwrap();

    let outcome = resolver(&root, credentials.clone()).resolve_startup();
    let StartupResolution::Recovery(recovery) = outcome.resolution else {
        panic!("expected Recovery for corrupt settings");
    };
    assert!(matches!(recovery.error(), StartupError::DamagedSettings(_)));
    // Non-destructive: the corrupt document is untouched and no migration ran,
    // so a retry keeps recovering against the same bytes.
    assert_eq!(std::fs::read(&settings_path).unwrap(), malformed);
    assert!(!root.path().join("accounts").exists());
    let retry = resolver(&root, credentials).resolve_startup();
    assert!(matches!(retry.resolution, StartupResolution::Recovery(_)));
    assert_eq!(std::fs::read(&settings_path).unwrap(), malformed);
}

/// Source guard: this test file never resolves a real local data directory or a
/// real credential backend. The forbidden names are assembled from fragments so
/// they never appear verbatim in the scanned source.
#[test]
fn test_sources_never_resolve_real_roots() {
    let source = include_str!("tests.rs");
    let default_root = format!("{}{}", "default_", "local");
    let local_dir = format!("{}{}", "data_local_", "dir");
    let real_store = format!("{}{}", "WindowsCredential", "Store");
    assert!(
        !source.contains(&default_root),
        "tests must not resolve the default local storage root"
    );
    assert!(
        !source.contains(&local_dir),
        "tests must not resolve the OS local data directory"
    );
    assert!(
        !source.contains(&real_store),
        "tests must not use the real Windows credential store"
    );
}

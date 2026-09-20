//! Flat-layout migration tests. All roots are explicit tempfile paths and all
//! credentials are in-memory or failing; nothing resolves real LocalAppData.

use std::path::Path;
use std::sync::Arc;

use chrono::Utc;
use rusqlite::Connection;
use tempfile::TempDir;

use super::*;
use crate::account::{AccountRegistryStore, InMemoryCredentialStore, RegistryMode};
use crate::storage::StorageRoot;
use crate::vault::AccountData;

fn root() -> (TempDir, StorageRoot) {
    let temp = tempfile::tempdir().unwrap();
    let root = StorageRoot::from_path(temp.path()).unwrap();
    (temp, root)
}

fn write(root: &StorageRoot, relative: &str, bytes: &[u8]) {
    let path = root.path().join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn seed_loot_db(root: &StorageRoot, relative: &str) {
    let path = root.path().join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let conn = Connection::open(&path).unwrap();
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
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE fights (id INTEGER PRIMARY KEY);
         CREATE TABLE fight_participants (fight_id INTEGER, object_id INTEGER);",
    )
    .unwrap();
}

fn seed_known_flat_layout(root: &StorageRoot, account_id: &str) {
    write(
        root,
        "settings.json",
        format!(r#"{{"account":{{"account_id":"{account_id}","account_name":"Hero","last_client_seen_unix":100,"last_client_launch_unix":200}}}}"#)
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
    write(
        root,
        "imports/dungeon.stats",
        br#"{"data":{"0":{"name":"Shatters","enteredDungeon":1,"totalTime":10}}}"#,
    );
    write(root, "logs/chat/2026-08-01.log", b"hello world");
    write(root, "logs/app.log", b"global app log");
    write(root, "event_notification_log.csv", b"a,b,c\n");
    write(root, "watchlist_detections.log", b"detection\n");
    write(root, "watchlist.txt", b"player\n");
    write(root, "captures/unassigned/x.pcap", b"capture");
    write(root, "assets/sprites/x.png", b"sprite");
    write(root, "sounds/custom/x.wav", b"sound");
    write(root, "access_token.txt", b"secret-token-value");
    seed_loot_db(root, "loot_history.db");
    seed_combat_db(root, "combat_history.db");
}

fn migration(
    root: &StorageRoot,
    credentials: Arc<dyn crate::account::CredentialStore>,
) -> FlatLayoutMigration {
    FlatLayoutMigration::new(root.clone(), None, credentials)
}

#[test]
fn known_complete_migration_selects_profile_and_imports_token() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");
    let credentials = Arc::new(InMemoryCredentialStore::new());
    let outcome = migration(&root, credentials.clone())
        .run_at(Utc::now())
        .unwrap();

    let account_key = match outcome {
        MigrationOutcome::ReadyKnown { account_key, .. } => account_key,
        other => panic!("expected ReadyKnown, got {other:?}"),
    };

    // Registry selected the migrated profile.
    let store = AccountRegistryStore::new(root.clone());
    let registry = store.reconcile().unwrap().registry;
    assert_eq!(registry.mode(), RegistryMode::Selected);
    assert_eq!(registry.selected_account_key(), Some(account_key));
    let entry = registry.entry(account_key).unwrap();
    assert_eq!(entry.account_id().as_str(), "ACCT-1");
    assert_eq!(entry.last_client_seen_unix(), 100);
    assert_eq!(entry.last_client_launch_unix(), 200);

    // Token imported into secure storage; plaintext removed.
    let target = crate::account::credential_target(account_key);
    assert_eq!(
        credentials.read(&target).unwrap().unwrap().token(),
        "secret-token-value"
    );
    assert!(!root.path().join("access_token.txt").exists());

    // Profile files exist.
    let profile = root.path().join("accounts").join(account_key.to_string());
    assert!(profile.join("account_data.json").exists());
    assert!(profile.join("quests").join("quests.json").exists());
    assert!(profile.join("cache").join("char_list.xml").exists());
    assert!(profile.join("databases").join("loot_history.db").exists());
    assert!(profile.join("databases").join("combat_history.db").exists());
    assert!(profile
        .join("logs")
        .join("chat")
        .join("2026-08-01.log")
        .exists());

    // Global data untouched.
    assert!(root.path().join("watchlist.txt").exists());
    assert!(root
        .path()
        .join("captures")
        .join("unassigned")
        .join("x.pcap")
        .exists());
    assert!(root
        .path()
        .join("assets")
        .join("sprites")
        .join("x.png")
        .exists());
    assert!(root
        .path()
        .join("sounds")
        .join("custom")
        .join("x.wav")
        .exists());
    assert!(root.path().join("logs").join("app.log").exists());

    // Originals relocated into a timestamped backup; token never backed up.
    let backups = root.path().join("migration-backups");
    assert!(backups.exists());
    let backup_dir = std::fs::read_dir(&backups)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(backup_dir.join("account_data.json").exists());
    assert!(backup_dir.join("loot_history.db").exists());
    assert!(!contains_token(&backup_dir, "secret-token-value"));
}

fn contains_token(dir: &Path, token: &str) -> bool {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if contains_token(&path, token) {
                return true;
            }
        } else if let Ok(bytes) = std::fs::read(&path) {
            if String::from_utf8_lossy(&bytes).contains(token) {
                return true;
            }
        }
    }
    false
}

/// A credential store whose writes always fail, to prove secure-store failure
/// degrades to a missing token without invalidating the migration.
#[derive(Debug, Default)]
struct FailingCredentialStore;

impl crate::account::CredentialStore for FailingCredentialStore {
    fn read(
        &self,
        _target: &str,
    ) -> Result<Option<crate::account::SavedCredential>, crate::account::CredentialError> {
        Ok(None)
    }
    fn write(
        &self,
        _target: &str,
        _credential: &crate::account::SavedCredential,
    ) -> Result<(), crate::account::CredentialError> {
        Err(crate::account::CredentialError::Unavailable)
    }
    fn delete(&self, _target: &str) -> Result<(), crate::account::CredentialError> {
        Ok(())
    }
}

fn only_backup_dir(root: &StorageRoot) -> std::path::PathBuf {
    let backups = root.path().join("migration-backups");
    std::fs::read_dir(&backups)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
}

#[test]
fn unknown_migration_quarantines_and_awaits_attribution() {
    let (_temp, root) = root();
    // No account id in settings => unknown/discovery.
    write(&root, "settings.json", br#"{"account":{}}"#);
    write(
        &root,
        "account_data.json",
        &serde_json::to_vec(&AccountData::new()).unwrap(),
    );
    write(&root, "quests.json", br#"{"quests":[]}"#);
    write(&root, "access_token.txt", b"unknown-token");
    seed_loot_db(&root, "loot_history.db");

    let credentials = Arc::new(InMemoryCredentialStore::new());
    let outcome = migration(&root, credentials.clone())
        .run_at(Utc::now())
        .unwrap();
    assert!(matches!(
        outcome,
        MigrationOutcome::AwaitingAttribution { .. }
    ));

    // Discovery mode committed; no account selected.
    let registry = AccountRegistryStore::new(root.clone())
        .reconcile()
        .unwrap()
        .registry;
    assert_eq!(registry.mode(), RegistryMode::Discovering);
    assert_eq!(registry.selected_account_key(), None);

    // Quarantine holds validated copies.
    let quarantine = root.path().join("quarantine").join("legacy-unassigned");
    assert!(quarantine.join("account_data.json").exists());
    assert!(quarantine.join("quests.json").exists());
    assert!(quarantine.join("loot_history.db").exists());

    // The token is never quarantined or backed up and the original remains.
    assert!(!quarantine.join("access_token.txt").exists());
    assert!(root.path().join("access_token.txt").exists());
    let backup_dir = only_backup_dir(&root);
    assert!(!contains_token(&backup_dir, "unknown-token"));
    // No credential written for an unknown account.
    assert!(credentials
        .read("RealmHound/account/anything")
        .unwrap()
        .is_none());
}

#[test]
fn partial_known_migration_handles_missing_sources() {
    let (_temp, root) = root();
    write(
        &root,
        "settings.json",
        br#"{"account":{"account_id":"ACCT-P"}}"#,
    );
    // Only account_data.json is present; everything else is absent.
    write(
        &root,
        "account_data.json",
        &serde_json::to_vec(&AccountData::new()).unwrap(),
    );

    let outcome = migration(&root, Arc::new(InMemoryCredentialStore::new()))
        .run_at(Utc::now())
        .unwrap();
    let account_key = match outcome {
        MigrationOutcome::ReadyKnown { account_key, .. } => account_key,
        other => panic!("expected ReadyKnown, got {other:?}"),
    };
    let profile = root.path().join("accounts").join(account_key.to_string());
    assert!(profile.join("account_data.json").exists());
    assert!(!profile.join("databases").join("loot_history.db").exists());
}

#[test]
fn contradictory_char_list_is_quarantined() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");
    // Overwrite char_list with a contradictory embedded owner id.
    write(
        &root,
        "char_list.xml",
        br#"<Chars><Account><AccountId>OTHER</AccountId></Account></Chars>"#,
    );

    let outcome = migration(&root, Arc::new(InMemoryCredentialStore::new()))
        .run_at(Utc::now())
        .unwrap();
    let account_key = match outcome {
        MigrationOutcome::ReadyKnown { account_key, .. } => account_key,
        other => panic!("expected ReadyKnown, got {other:?}"),
    };
    let profile = root.path().join("accounts").join(account_key.to_string());
    // Not in the profile cache; instead quarantined.
    assert!(!profile.join("cache").join("char_list.xml").exists());
    assert!(root
        .path()
        .join("quarantine")
        .join("legacy-unassigned")
        .join("char_list.xml")
        .exists());
}

#[test]
fn contradictory_account_data_falls_back_and_quarantines() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");
    // account_data.json embeds a different owner id.
    write(
        &root,
        "account_data.json",
        br#"{"version":1,"characters":{},"account_id":"SOMEONE-ELSE"}"#,
    );

    let outcome = migration(&root, Arc::new(InMemoryCredentialStore::new()))
        .run_at(Utc::now())
        .unwrap();
    let account_key = match outcome {
        MigrationOutcome::ReadyKnown { account_key, .. } => account_key,
        other => panic!("expected ReadyKnown, got {other:?}"),
    };
    // The contradictory source is quarantined; the profile still gets a
    // (fallback-built) account_data.json.
    let profile = root.path().join("accounts").join(account_key.to_string());
    assert!(profile.join("account_data.json").exists());
    assert!(root
        .path()
        .join("quarantine")
        .join("legacy-unassigned")
        .join("account_data.json")
        .exists());
}

#[test]
fn corrupt_json_source_fails_without_moving_originals() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");
    write(&root, "quests.json", b"{ this is not json");

    let error = migration(&root, Arc::new(InMemoryCredentialStore::new()))
        .run_at(Utc::now())
        .unwrap_err();
    assert!(matches!(error, MigrationError::JsonInvalid { .. }));
    // Originals untouched: nothing relocated, registry not selected.
    assert!(root.path().join("quests.json").exists());
    assert!(root.path().join("account_data.json").exists());
    let registry = AccountRegistryStore::new(root.clone())
        .reconcile()
        .unwrap()
        .registry;
    assert_eq!(registry.mode(), RegistryMode::Discovering);
}

#[test]
fn future_version_database_blocks_and_resumes_after_fix() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");
    // Rewrite loot db with an unsupported future version.
    let loot = root.path().join("loot_history.db");
    std::fs::remove_file(&loot).ok();
    {
        let conn = Connection::open(&loot).unwrap();
        conn.execute_batch(&format!(
            "CREATE TABLE loot_drops (id INTEGER PRIMARY KEY);
             CREATE TABLE loot_items (id INTEGER PRIMARY KEY);
             PRAGMA user_version = {};",
            crate::loot::SUPPORTED_SCHEMA_VERSION + 5
        ))
        .unwrap();
    }

    let error = migration(&root, Arc::new(InMemoryCredentialStore::new()))
        .run_at(Utc::now())
        .unwrap_err();
    assert!(matches!(
        error,
        MigrationError::DatabaseVersionUnsupported { .. }
    ));
    // Originals remain authoritative; registry not committed.
    assert!(root.path().join("account_data.json").exists());
    let registry = AccountRegistryStore::new(root.clone())
        .reconcile()
        .unwrap()
        .registry;
    assert_eq!(registry.mode(), RegistryMode::Discovering);

    // Fix the database and resume: the migration completes from the journal.
    std::fs::remove_file(&loot).unwrap();
    seed_loot_db(&root, "loot_history.db");
    let outcome = migration(&root, Arc::new(InMemoryCredentialStore::new()))
        .run_at(Utc::now())
        .unwrap();
    assert!(matches!(outcome, MigrationOutcome::ReadyKnown { .. }));
    let registry = AccountRegistryStore::new(root.clone())
        .reconcile()
        .unwrap()
        .registry;
    assert_eq!(registry.mode(), RegistryMode::Selected);
}

#[test]
fn lock_contention_returns_blocked() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");
    let held = super::lock::MigrationLock::acquire_root(&root).unwrap();

    let outcome = migration(&root, Arc::new(InMemoryCredentialStore::new()))
        .run_at(Utc::now())
        .unwrap();
    assert!(matches!(outcome, MigrationOutcome::Blocked { .. }));
    // No migration source changed.
    assert!(root.path().join("account_data.json").exists());
    drop(held);
}

#[test]
fn completed_migration_rerun_is_idempotent() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");
    let credentials = Arc::new(InMemoryCredentialStore::new());
    let first = migration(&root, credentials.clone())
        .run_at(Utc::now())
        .unwrap();
    let first_key = match first {
        MigrationOutcome::ReadyKnown { account_key, .. } => account_key,
        other => panic!("expected ReadyKnown, got {other:?}"),
    };

    let second = migration(&root, credentials).run_at(Utc::now()).unwrap();
    assert_eq!(
        second,
        MigrationOutcome::AlreadyComplete {
            account_key: Some(first_key)
        }
    );
}

#[test]
fn token_secure_store_failure_still_completes_and_removes_plaintext() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");

    let outcome = migration(&root, Arc::new(FailingCredentialStore))
        .run_at(Utc::now())
        .unwrap();
    let account_key = match outcome {
        MigrationOutcome::ReadyKnown { account_key, .. } => account_key,
        other => panic!("expected ReadyKnown, got {other:?}"),
    };
    // Migration completes; no credential target recorded; plaintext removed.
    let registry = AccountRegistryStore::new(root.clone())
        .reconcile()
        .unwrap()
        .registry;
    assert_eq!(
        registry.entry(account_key).unwrap().credential_target(),
        None
    );
    assert!(!root.path().join("access_token.txt").exists());
}

#[test]
fn roaming_cache_imported_and_external_original_untouched() {
    let (_temp, root) = root();
    let roaming_temp = tempfile::tempdir().unwrap();
    let roaming = StorageRoot::from_path(roaming_temp.path()).unwrap();

    write(
        &root,
        "settings.json",
        br#"{"account":{"account_id":"ACCT-R"}}"#,
    );
    // No local account_data or local characters cache; only Roaming cache.
    let mut cache = crate::vault::CharacterCache::default();
    cache.custom_labels.insert(9, "Roamer".to_string());
    std::fs::write(
        roaming.path().join("characters_cache.json"),
        serde_json::to_vec(&cache).unwrap(),
    )
    .unwrap();

    let outcome = FlatLayoutMigration::new(
        root.clone(),
        Some(roaming.clone()),
        Arc::new(InMemoryCredentialStore::new()),
    )
    .run_at(Utc::now())
    .unwrap();
    let account_key = match outcome {
        MigrationOutcome::ReadyKnown { account_key, .. } => account_key,
        other => panic!("expected ReadyKnown, got {other:?}"),
    };

    // The external Roaming original is never moved.
    assert!(roaming.path().join("characters_cache.json").exists());
    // The imported Roaming cache is preserved in a distinct backup path.
    let backup_dir = only_backup_dir(&root);
    assert!(backup_dir
        .join("roaming")
        .join("characters_cache.json")
        .exists());
    // The profile account_data was built from the Roaming cache.
    let profile = root.path().join("accounts").join(account_key.to_string());
    let data: AccountData =
        serde_json::from_slice(&std::fs::read(profile.join("account_data.json")).unwrap()).unwrap();
    assert_eq!(
        data.characters.custom_labels.get(&9).map(String::as_str),
        Some("Roamer")
    );
}

#[cfg(unix)]
#[test]
fn symlinked_source_is_rejected() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");
    // Replace quests.json with a symlink to an outside file.
    let outside = root.path().join("outside.json");
    std::fs::write(&outside, br#"{"quests":[]}"#).unwrap();
    std::fs::remove_file(root.path().join("quests.json")).unwrap();
    std::os::unix::fs::symlink(&outside, root.path().join("quests.json")).unwrap();

    let error = migration(&root, Arc::new(InMemoryCredentialStore::new()))
        .run_at(Utc::now())
        .unwrap_err();
    assert!(matches!(error, MigrationError::Storage(_)));
}

// ---------------------------------------------------------------------------
// Cheap, lock-free pending() pre-check.
// ---------------------------------------------------------------------------

#[test]
fn pending_true_before_migration_and_false_after_completion() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");
    let creds = Arc::new(InMemoryCredentialStore::new());

    // No journal yet: migration is pending.
    assert!(migration(&root, creds.clone()).pending());

    migration(&root, creds.clone()).run_at(Utc::now()).unwrap();

    // Completed journal: nothing to do.
    assert!(!migration(&root, creds).pending());
}

#[test]
fn pending_true_when_journal_incomplete() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");
    let creds = Arc::new(InMemoryCredentialStore::new());

    run_until(&root, creds.clone(), "registry-committed");
    assert!(migration(&root, creds).pending());
}

// ---------------------------------------------------------------------------
// Helpers for interruption/resume and journal inspection.
// ---------------------------------------------------------------------------

fn read_journal(root: &StorageRoot) -> super::journal::MigrationJournal {
    super::journal::load_journal(root).unwrap().unwrap()
}

fn run_until(root: &StorageRoot, creds: Arc<dyn crate::account::CredentialStore>, at: &str) {
    let migration = migration(root, creds);
    migration.arm_failpoint(at);
    let error = migration.run_at(Utc::now()).unwrap_err();
    match error {
        MigrationError::Interrupted { at: hit } => assert_eq!(hit, at),
        other => panic!("expected interruption at `{at}`, got {other:?}"),
    }
}

fn profile_dir(root: &StorageRoot, key: AccountKey) -> std::path::PathBuf {
    root.path().join("accounts").join(key.to_string())
}

// ---------------------------------------------------------------------------
// Requirement 15/10: genuine interruption at each persisted boundary.
// ---------------------------------------------------------------------------

#[test]
fn resumes_cleanly_after_interruption_at_each_boundary() {
    for boundary in [
        "prepared",
        "copy:loot_history.db",
        "copies-validated",
        "registry-committed",
        "backup:quests.json",
        "originals-backed-up",
    ] {
        let (_temp, root) = root();
        seed_known_flat_layout(&root, "ACCT-1");
        let creds = Arc::new(InMemoryCredentialStore::new());

        run_until(&root, creds.clone(), boundary);
        // The persisted journal never uses a SQLite substate; only the five
        // known states plus the unknown terminal are representable.
        let journal = read_journal(&root);
        assert!(matches!(
            journal.phase,
            MigrationPhase::Prepared
                | MigrationPhase::CopiesValidated
                | MigrationPhase::RegistryCommitted
                | MigrationPhase::OriginalsBackedUp
        ));

        // A fresh run resumes from the journal and completes.
        let outcome = migration(&root, creds.clone()).run_at(Utc::now()).unwrap();
        let key = match outcome {
            MigrationOutcome::ReadyKnown { account_key, .. } => account_key,
            other => panic!("[{boundary}] expected ReadyKnown, got {other:?}"),
        };
        let profile = profile_dir(&root, key);
        assert!(profile.join("account_data.json").exists(), "[{boundary}]");
        assert!(
            profile.join("databases").join("loot_history.db").exists(),
            "[{boundary}]"
        );
        assert!(
            profile.join("quests").join("quests.json").exists(),
            "[{boundary}]"
        );
        assert!(
            !root.path().join("access_token.txt").exists(),
            "[{boundary}]"
        );
    }
}

// ---------------------------------------------------------------------------
// Requirement 1: interrupted opaque-tree copy leaves an unjournaled partial
// destination that must be safely removed and recopied.
// ---------------------------------------------------------------------------

#[test]
fn partial_opaque_tree_is_removed_and_recopied() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");
    let creds = Arc::new(InMemoryCredentialStore::new());

    // Interrupt mid-copy, after the profile manifest exists but before the chat
    // log tree has been copied and journaled.
    run_until(&root, creds.clone(), "copy:loot_history.db");
    let key = read_journal(&root).target_account_key.unwrap();

    // Simulate a partially-copied chat log tree that was never journaled.
    let chat = profile_dir(&root, key).join("logs").join("chat");
    std::fs::create_dir_all(&chat).unwrap();
    std::fs::write(chat.join("garbage-partial.log"), b"stale partial").unwrap();

    // Resume: the partial tree is removed and the real source recopied exactly.
    let outcome = migration(&root, creds).run_at(Utc::now()).unwrap();
    assert!(matches!(outcome, MigrationOutcome::ReadyKnown { .. }));
    assert!(chat.join("2026-08-01.log").exists());
    assert!(
        !chat.join("garbage-partial.log").exists(),
        "unjournaled partial must be removed before recopy"
    );
}

// ---------------------------------------------------------------------------
// Requirement 3: backup-only sources are copied+validated then deleted.
// ---------------------------------------------------------------------------

#[test]
fn backup_only_sources_are_copied_validated_then_deleted() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");
    write(&root, "party_sightings.json", b"orphan-data");
    write(&root, "debug/api/snapshot.json", b"debug-snapshot");

    let outcome = migration(&root, Arc::new(InMemoryCredentialStore::new()))
        .run_at(Utc::now())
        .unwrap();
    assert!(matches!(outcome, MigrationOutcome::ReadyKnown { .. }));

    let backup = only_backup_dir(&root);
    assert!(backup.join("party_sightings.json").exists());
    assert!(backup
        .join("debug")
        .join("api")
        .join("snapshot.json")
        .exists());
    // Originals removed only after the validated backup copy exists.
    assert!(!root.path().join("party_sightings.json").exists());
    assert!(!root.path().join("debug").exists());
}

#[test]
fn backup_only_interruption_between_copy_and_delete_is_recoverable() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");
    write(&root, "party_snapshot.json", b"orphan-snapshot");
    let creds = Arc::new(InMemoryCredentialStore::new());

    // Interrupt after the copy phase: the backup copy exists but the original
    // has not yet been deleted.
    run_until(&root, creds.clone(), "copies-validated");
    let backup_dir = read_journal(&root).backup_directory.clone();
    assert!(root
        .path()
        .join(&backup_dir)
        .join("party_snapshot.json")
        .exists());
    assert!(root.path().join("party_snapshot.json").exists());

    // Resume completes: the original is deleted after revalidation.
    migration(&root, creds).run_at(Utc::now()).unwrap();
    assert!(!root.path().join("party_snapshot.json").exists());
}

// ---------------------------------------------------------------------------
// Requirement 4: strict token parsing.
// ---------------------------------------------------------------------------

#[test]
fn json_token_import_preserves_captured_at() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");
    let ts = "2026-08-07T10:00:00Z";
    write(
        &root,
        "access_token.txt",
        format!(r#"{{"token":"json-secret","captured_at":"{ts}"}}"#).as_bytes(),
    );
    let creds = Arc::new(InMemoryCredentialStore::new());

    let outcome = migration(&root, creds.clone()).run_at(Utc::now()).unwrap();
    let key = match outcome {
        MigrationOutcome::ReadyKnown { account_key, .. } => account_key,
        other => panic!("expected ReadyKnown, got {other:?}"),
    };
    let target = crate::account::credential_target(key);
    let saved = creds.read(&target).unwrap().unwrap();
    assert_eq!(saved.token(), "json-secret");
    assert_eq!(
        saved.captured_at(),
        Some(ts.parse::<chrono::DateTime<Utc>>().unwrap()),
        "captured_at must be preserved, not replaced with now"
    );
}

#[test]
fn bare_token_import_has_unknown_captured_at() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");
    write(&root, "access_token.txt", b"legacy-bare-token\n");
    let creds = Arc::new(InMemoryCredentialStore::new());

    let outcome = migration(&root, creds.clone()).run_at(Utc::now()).unwrap();
    let key = match outcome {
        MigrationOutcome::ReadyKnown { account_key, .. } => account_key,
        other => panic!("expected ReadyKnown, got {other:?}"),
    };
    let target = crate::account::credential_target(key);
    let saved = creds.read(&target).unwrap().unwrap();
    assert_eq!(saved.token(), "legacy-bare-token");
    assert!(saved.captured_at().is_none());
}

#[test]
fn malformed_token_blocks_and_remains() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");
    write(&root, "access_token.txt", br#"{"token":"trunc"#);

    let error = migration(&root, Arc::new(InMemoryCredentialStore::new()))
        .run_at(Utc::now())
        .unwrap_err();
    assert!(matches!(error, MigrationError::TokenMalformed));
    // The malformed token file remains and the registry is not committed.
    assert!(root.path().join("access_token.txt").exists());
    let registry = AccountRegistryStore::new(root.clone())
        .reconcile()
        .unwrap()
        .registry;
    assert_eq!(registry.mode(), RegistryMode::Discovering);
}

// ---------------------------------------------------------------------------
// Requirement 5: present caches/vault are typed-validated (no silent ignore).
// ---------------------------------------------------------------------------

#[test]
fn corrupt_local_characters_cache_fails_without_moving_originals() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");
    write(&root, "characters_cache.json", b"{ not json");

    let error = migration(&root, Arc::new(InMemoryCredentialStore::new()))
        .run_at(Utc::now())
        .unwrap_err();
    assert!(matches!(error, MigrationError::JsonInvalid { .. }));
    assert!(root.path().join("characters_cache.json").exists());
    assert!(root.path().join("account_data.json").exists());
}

#[test]
fn corrupt_live_vault_fails_without_moving_originals() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");
    write(&root, "live_vault.json", b"{ broken vault");

    let error = migration(&root, Arc::new(InMemoryCredentialStore::new()))
        .run_at(Utc::now())
        .unwrap_err();
    assert!(matches!(error, MigrationError::JsonInvalid { .. }));
    assert!(root.path().join("live_vault.json").exists());
}

#[test]
fn corrupt_roaming_cache_fails_without_moving_originals() {
    let (_temp, root) = root();
    let roaming_temp = tempfile::tempdir().unwrap();
    let roaming = StorageRoot::from_path(roaming_temp.path()).unwrap();
    write(
        &root,
        "settings.json",
        br#"{"account":{"account_id":"ACCT-R"}}"#,
    );
    std::fs::write(roaming.path().join("characters_cache.json"), b"{ not valid").unwrap();

    let error = FlatLayoutMigration::new(
        root.clone(),
        Some(roaming.clone()),
        Arc::new(InMemoryCredentialStore::new()),
    )
    .run_at(Utc::now())
    .unwrap_err();
    assert!(matches!(error, MigrationError::JsonInvalid { .. }));
    assert!(roaming.path().join("characters_cache.json").exists());
}

// ---------------------------------------------------------------------------
// Requirement 7: destinations are revalidated, not trusted, on resume.
// ---------------------------------------------------------------------------

#[test]
fn corrupt_profile_destination_is_recreated_on_resume() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");
    let creds = Arc::new(InMemoryCredentialStore::new());

    run_until(&root, creds.clone(), "copies-validated");
    let key = read_journal(&root).target_account_key.unwrap();
    let profile = profile_dir(&root, key);

    // Corrupt a validated file destination and delete a validated database.
    std::fs::write(profile.join("quests").join("quests.json"), b"corrupted").unwrap();
    std::fs::remove_file(profile.join("databases").join("loot_history.db")).unwrap();

    // Resume: revalidation detects the damage and recreates both.
    migration(&root, creds).run_at(Utc::now()).unwrap();
    assert_eq!(
        std::fs::read(profile.join("quests").join("quests.json")).unwrap(),
        br#"{"quests":[]}"#
    );
    let conn = Connection::open(profile.join("databases").join("loot_history.db")).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM loot_drops", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

// ---------------------------------------------------------------------------
// Requirement 8: journal item ids must equal inventory; fail closed otherwise.
// ---------------------------------------------------------------------------

#[test]
fn tampered_journal_with_missing_item_fails_closed() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");
    let creds = Arc::new(InMemoryCredentialStore::new());
    run_until(&root, creds.clone(), "prepared");

    // Drop an inventory item from the journal on disk.
    let mut journal = read_journal(&root);
    journal.items.retain(|item| item.id != "quests.json");
    super::journal::save_journal(&root, &journal).unwrap();

    let error = migration(&root, creds).run_at(Utc::now()).unwrap_err();
    assert!(matches!(error, MigrationError::JournalMismatch { .. }));
}

// ---------------------------------------------------------------------------
// Requirement 9: identity binding detects a settings identity change.
// ---------------------------------------------------------------------------

#[test]
fn settings_identity_change_after_prepared_fails_closed() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");
    let creds = Arc::new(InMemoryCredentialStore::new());
    run_until(&root, creds.clone(), "prepared");

    // The user's settings now claim a different account identity.
    write(
        &root,
        "settings.json",
        br#"{"account":{"account_id":"ACCT-DIFFERENT"}}"#,
    );

    let error = migration(&root, creds).run_at(Utc::now()).unwrap_err();
    assert!(matches!(error, MigrationError::JournalMismatch { .. }));
}

// ---------------------------------------------------------------------------
// Requirement 10: post-commit resume must not parse mutable/corrupt settings.
// ---------------------------------------------------------------------------

#[test]
fn resume_after_commit_ignores_corrupt_settings() {
    let (_temp, root) = root();
    seed_known_flat_layout(&root, "ACCT-1");
    let creds = Arc::new(InMemoryCredentialStore::new());

    // Interrupt immediately after the registry commit is journaled.
    run_until(&root, creds.clone(), "registry-committed");
    assert_eq!(read_journal(&root).phase, MigrationPhase::RegistryCommitted);

    // Corrupt settings after the commit; a post-commit resume must not read it.
    write(&root, "settings.json", b"{ corrupt settings");

    let outcome = migration(&root, creds).run_at(Utc::now()).unwrap();
    assert!(matches!(outcome, MigrationOutcome::ReadyKnown { .. }));
    let registry = AccountRegistryStore::new(root.clone())
        .reconcile()
        .unwrap()
        .registry;
    assert_eq!(registry.mode(), RegistryMode::Selected);
}

// ---------------------------------------------------------------------------
// Requirement 11: preexisting quarantine blocks a brand-new migration, but a
// migration-owned partial quarantine is retryable once Prepared is journaled.
// ---------------------------------------------------------------------------

#[test]
fn preexisting_nonempty_quarantine_blocks_new_migration() {
    let (_temp, root) = root();
    write(&root, "settings.json", br#"{"account":{}}"#);
    write(&root, "account_data.json", b"{}");
    // Foreign quarantine content present before any journal exists.
    write(
        &root,
        "quarantine/legacy-unassigned/foreign.json",
        b"not ours",
    );

    let outcome = migration(&root, Arc::new(InMemoryCredentialStore::new()))
        .run_at(Utc::now())
        .unwrap();
    assert!(matches!(outcome, MigrationOutcome::Blocked { .. }));
    // No journal was created; the source is untouched.
    assert!(super::journal::load_journal(&root).unwrap().is_none());
}

#[test]
fn migration_owned_partial_quarantine_is_retryable() {
    let (_temp, root) = root();
    write(&root, "settings.json", br#"{"account":{}}"#);
    write(
        &root,
        "account_data.json",
        &serde_json::to_vec(&AccountData::new()).unwrap(),
    );
    write(&root, "quests.json", br#"{"quests":[]}"#);
    let creds = Arc::new(InMemoryCredentialStore::new());

    // Interrupt after Prepared: the quarantine is now migration-owned.
    run_until(&root, creds.clone(), "prepared");
    // A partial exact destination that migration itself may have written.
    write(
        &root,
        "quarantine/legacy-unassigned/account_data.json",
        b"partial",
    );

    let outcome = migration(&root, creds).run_at(Utc::now()).unwrap();
    assert!(matches!(
        outcome,
        MigrationOutcome::AwaitingAttribution { .. }
    ));
    assert!(root
        .path()
        .join("quarantine")
        .join("legacy-unassigned")
        .join("quests.json")
        .exists());
}

//! Source guard: account-scoped persistence modules must not resolve
//! `dirs::data_local_dir()` themselves.
//!
//! All path resolution is centralized in the `StorageRoot` /
//! `AccountPersistencePaths` / `AccountPaths` layer and injected into these
//! modules. The centralized compatibility layer (`storage::StorageRoot`,
//! `account::persistence`) and documented global paths (settings, captures,
//! application logs, generated asset caches) are intentionally excluded.

use std::path::Path;

/// Account-scoped consumer modules that must never resolve LocalAppData
/// directly. Each receives injected paths instead.
const GUARDED: &[&str] = &[
    "src/vault/account_data.rs",
    "src/vault/character_cache.rs",
    "src/loot/database.rs",
    "src/combat/database.rs",
    "src/api/token.rs",
    "src/api/client.rs",
    "src/realmshark_import.rs",
    "src/watchlist.rs",
    "src/account/registry.rs",
    "src/account/credentials.rs",
    "src/account/lock.rs",
    "src/account/startup.rs",
    "src/account/migration/mod.rs",
    "src/account/migration/settings_view.rs",
    "src/account/migration/sqlite.rs",
    "src/account/migration/validate.rs",
    "src/account/migration/journal.rs",
    "src/account/migration/lock.rs",
    "src/account/migration/backup.rs",
    "src/account/migration/inventory.rs",
];

/// Whether a source line is a comment (skipped so doc examples that mention a
/// path do not trip the guard).
fn is_comment(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("//") || trimmed.starts_with('*') || trimmed.starts_with("/*")
}

#[test]
fn account_scoped_modules_do_not_resolve_localappdata() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for rel in GUARDED {
        let path = root.join(rel);
        let contents = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {rel}: {e}"));
        for (index, line) in contents.lines().enumerate() {
            if is_comment(line) {
                continue;
            }
            assert!(
                !line.contains("data_local_dir"),
                "{rel}:{} resolves data_local_dir; inject the path instead",
                index + 1
            );
            assert!(
                !line.contains("\"LOCALAPPDATA\"") && !line.contains("\"APPDATA\""),
                "{rel}:{} reads a LocalAppData/Roaming env var; inject the path instead",
                index + 1
            );
        }
    }
}

/// The testable startup path resolves nothing itself: it never touches the
/// default root adapter, the flat compatibility layout, or the legacy plaintext
/// token helpers. The production default-root adapter lives only in the binary's
/// `main`, outside these modules.
const STARTUP_GUARDED: &[&str] = &["src/account/startup.rs", "src/account/lock.rs"];

/// Fragments the startup path must never reference (assembled so the guard's own
/// source does not trip it).
fn startup_forbidden() -> Vec<String> {
    vec![
        format!("{}{}", "default_", "local"),
        format!("{}{}", "data_local_", "dir"),
        format!("{}{}", "flat_", "compat"),
        format!("{}{}", "legacy_access_", "token"),
        format!("{}{}", "load_saved_", "token"),
        format!("{}{}", "save_", "token"),
    ]
}

#[test]
fn startup_path_never_resolves_default_root_or_legacy_helpers() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let forbidden = startup_forbidden();
    for rel in STARTUP_GUARDED {
        let path = root.join(rel);
        let contents = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {rel}: {e}"));
        for (index, line) in contents.lines().enumerate() {
            if is_comment(line) {
                continue;
            }
            for needle in &forbidden {
                assert!(
                    !line.contains(needle.as_str()),
                    "{rel}:{} references `{needle}`; the startup path must resolve nothing itself",
                    index + 1
                );
            }
        }
    }
}

//! Source guard: account-scoped UI panels must not resolve
//! `dirs::data_local_dir()` themselves.
//!
//! Panels receive their per-account paths from `AccountPersistencePaths`.
//! Documented global paths -- application logs (`main.rs`), custom sounds
//! (`sound.rs`), and the Oryx taunt capture / event-notification *global* opt-in
//! (`event_log.rs`) -- are intentionally excluded.

use std::path::Path;

/// Account-scoped panels that must never resolve LocalAppData directly.
const GUARDED: &[&str] = &[
    "src/panels/vault.rs",
    "src/panels/quest.rs",
    "src/panels/chat.rs",
    "src/panels/trophy_hall.rs",
];

fn is_comment(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("//") || trimmed.starts_with('*') || trimmed.starts_with("/*")
}

#[test]
fn account_scoped_panels_do_not_resolve_localappdata() {
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
        }
    }
}

#[test]
fn event_log_no_longer_resolves_the_account_notification_path() {
    // The per-account event-notification log path is injected; only the global
    // Oryx taunt capture (a documented global diagnostic) may resolve LocalAppData
    // in this module.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/event_log.rs");
    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(
        !contents.contains("event_notification_log.csv"),
        "event_log.rs must receive the account notification path, not resolve it"
    );
}

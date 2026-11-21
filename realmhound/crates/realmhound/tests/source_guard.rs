//! Source guard: account-scoped UI panels must not resolve
//! `dirs::data_local_dir()` themselves.
//!
//! Panels receive their per-account paths from `AccountPersistencePaths`.
//! Documented global paths -- application logs (`main.rs`) and custom sounds
//! (`sound.rs`) -- are intentionally excluded.

use std::path::Path;

/// Account-scoped panels that must never resolve LocalAppData directly.
const GUARDED: &[&str] = &[
    "src/panels/vault.rs",
    "src/panels/quest.rs",
    "src/panels/chat.rs",
    "src/panels/trophy_hall.rs",
];

#[test]
fn application_never_restores_or_persists_tokens() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for rel in ["src/app.rs", "src/main.rs", "src/discovery.rs"] {
        let source = std::fs::read_to_string(root.join(rel)).unwrap();
        for forbidden in [
            "SavedCredential",
            "read_credential",
            "save_account_credential",
            "load_saved_token",
            "save_token(",
        ] {
            assert!(!source.contains(forbidden), "{rel} references {forbidden}");
        }
    }
}

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

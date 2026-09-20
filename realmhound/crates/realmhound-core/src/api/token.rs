//! Access token persistence.
//!
//! The token captured from HELLO packets is stored at an explicitly injected
//! legacy path (`%LOCALAPPDATA%\RealmHound\access_token.txt`) as JSON
//! `{token, captured_at}`. Legacy plain-token files are still loaded, with an
//! unknown capture time. The path is injected so this module resolves no
//! machine-wide default; later phases move tokens to the credential store.

use std::fmt;
use std::fs;
use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// Assumed token lifetime. The server is authoritative; this only drives a
/// local heads-up before a request.
pub const TOKEN_VALIDITY_HOURS: i64 = 24;

/// A persisted access token and the time it was captured (`None` if unknown).
///
/// The token is redacted in `Debug` output so it can never be logged, matching
/// [`crate::account::SavedCredential`].
#[derive(Clone, Serialize, Deserialize)]
pub struct SavedToken {
    pub token: String,
    #[serde(default)]
    pub captured_at: Option<DateTime<Utc>>,
}

impl fmt::Debug for SavedToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SavedToken")
            .field("token", &"<redacted>")
            .field("captured_at", &self.captured_at)
            .finish()
    }
}

/// Strict outcome of parsing access-token file contents.
///
/// This is the reusable seam shared by token loading and flat-layout migration.
/// It distinguishes an absent token from malformed JSON so a caller can leave a
/// malformed file in place and retry rather than mistaking a truncated write for
/// "no token".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenParse {
    /// A usable token (current JSON `SavedToken` or a legacy bare token).
    Valid(SavedToken),
    /// The contents were empty or whitespace: no token is present.
    Empty,
    /// The contents began as a JSON object but were malformed, or the JSON
    /// carried an empty token. The file must not be treated as a token.
    Malformed,
}

impl PartialEq for SavedToken {
    fn eq(&self, other: &Self) -> bool {
        self.token == other.token && self.captured_at == other.captured_at
    }
}

impl Eq for SavedToken {}

/// Strictly parse token file contents, preserving `captured_at`.
///
/// Current documents are JSON `{token, captured_at}`; a legacy bare token has an
/// unknown capture time. Malformed JSON is reported as [`TokenParse::Malformed`]
/// rather than silently discarded, and the token contents are never logged.
pub fn parse_saved_token_strict(contents: &str) -> TokenParse {
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        return TokenParse::Empty;
    }

    if trimmed.starts_with('{') {
        match serde_json::from_str::<SavedToken>(trimmed) {
            Ok(saved) if !saved.token.trim().is_empty() => TokenParse::Valid(saved),
            // Reject malformed JSON (or an empty token) rather than mistaking a
            // truncated write for a token.
            _ => TokenParse::Malformed,
        }
    } else {
        TokenParse::Valid(SavedToken {
            token: trimmed.to_string(),
            captured_at: None,
        })
    }
}

/// Parse file contents into a [`SavedToken`]. Non-JSON input is treated as a
/// legacy bare token with an unknown capture time. Empty or malformed input
/// yields `None`.
fn parse_saved_token(contents: &str) -> Option<SavedToken> {
    match parse_saved_token_strict(contents) {
        TokenParse::Valid(saved) => Some(saved),
        TokenParse::Empty | TokenParse::Malformed => None,
    }
}

/// Load the saved token (and capture time) from the injected legacy path.
pub fn load_saved_token_full(path: &Path) -> Option<SavedToken> {
    match fs::read_to_string(path) {
        Ok(contents) => {
            let saved = parse_saved_token(&contents);
            if saved.is_some() {
                tracing::info!("[TOKEN] Loaded saved access token from {:?}", path);
            }
            saved
        }
        Err(_) => None,
    }
}

/// Load just the token string. Convenience wrapper around [`load_saved_token_full`].
pub fn load_saved_token(path: &Path) -> Option<String> {
    load_saved_token_full(path).map(|s| s.token)
}

/// Save the token and its capture time to the injected legacy path. Written
/// atomically (temp + rename) so a crash mid-write cannot leave a truncated
/// document.
pub fn save_token(path: &Path, token: &str, captured_at: Option<DateTime<Utc>>) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let saved = SavedToken {
        token: token.to_string(),
        captured_at,
    };
    let json = match serde_json::to_string(&saved) {
        Ok(json) => json,
        Err(e) => {
            tracing::warn!("[TOKEN] Failed to serialize token: {}", e);
            return;
        }
    };

    let tmp = path.with_extension("txt.tmp");
    if let Err(e) = fs::write(&tmp, &json) {
        tracing::warn!("[TOKEN] Failed to write token temp file: {}", e);
        return;
    }
    match fs::rename(&tmp, path) {
        Ok(_) => tracing::info!("[TOKEN] Saved access token to {:?}", path),
        Err(e) => {
            tracing::warn!("[TOKEN] Failed to persist token: {}", e);
            let _ = fs::remove_file(&tmp);
        }
    }
}

/// Age of a captured token, clamped to be non-negative. Returns `None` when the
/// capture time is unknown or implausibly in the future (clock change).
pub fn token_age(captured_at: Option<DateTime<Utc>>) -> Option<Duration> {
    let captured_at = captured_at?;
    let age = Utc::now() - captured_at;
    if age < Duration::minutes(-5) {
        None
    } else if age < Duration::zero() {
        Some(Duration::zero())
    } else {
        Some(age)
    }
}

/// Whether the token is past its assumed validity window. Unknown => `false`.
pub fn is_token_expired(captured_at: Option<DateTime<Utc>>) -> bool {
    match token_age(captured_at) {
        Some(age) => age >= Duration::hours(TOKEN_VALIDITY_HOURS),
        None => false,
    }
}

/// User-facing message for an invalid/expired token, including its age if known.
pub fn token_expiry_message(captured_at: Option<DateTime<Utc>>) -> String {
    match token_age(captured_at) {
        Some(age) => {
            let hours = age.num_hours();
            if hours >= 1 {
                format!(
                    "Access token invalid or expired (captured {hours}h ago) - launch the game to refresh."
                )
            } else {
                "Access token invalid or expired (captured under an hour ago) - launch the game to refresh."
                    .to_string()
            }
        }
        None => "Access token invalid or expired - launch the game to refresh.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_token_through_injected_path() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("access_token.txt");
        let captured = Some(Utc::now());
        save_token(&path, "abc123", captured);

        let saved = load_saved_token_full(&path).expect("token should load");
        assert_eq!(saved.token, "abc123");
        assert_eq!(load_saved_token(&path).as_deref(), Some("abc123"));
    }

    #[test]
    fn load_nonexistent_token_returns_none() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("missing_access_token.txt");
        assert!(load_saved_token(&path).is_none());
    }

    #[test]
    fn parses_json_document_with_timestamp() {
        let ts = "2026-08-07T10:00:00Z";
        let json = format!(r#"{{"token":"abc123","captured_at":"{}"}}"#, ts);
        let saved = parse_saved_token(&json).expect("should parse JSON token");
        assert_eq!(saved.token, "abc123");
        assert_eq!(
            saved.captured_at,
            Some(ts.parse::<DateTime<Utc>>().unwrap())
        );
    }

    #[test]
    fn parses_legacy_bare_token() {
        let saved = parse_saved_token("  legacy_raw_token\n").expect("should parse legacy token");
        assert_eq!(saved.token, "legacy_raw_token");
        assert!(
            saved.captured_at.is_none(),
            "legacy tokens have no capture time"
        );
    }

    #[test]
    fn rejects_malformed_json_object() {
        assert!(parse_saved_token(r#"{"token":"abc"#).is_none());
        assert!(parse_saved_token("{}").is_none());
    }

    #[test]
    fn rejects_empty_contents() {
        assert!(parse_saved_token("   \n").is_none());
    }

    #[test]
    fn strict_seam_distinguishes_valid_empty_and_malformed() {
        // Current JSON document with a preserved timestamp.
        let ts = "2026-08-07T10:00:00Z";
        let json = format!(r#"{{"token":"abc123","captured_at":"{}"}}"#, ts);
        match parse_saved_token_strict(&json) {
            TokenParse::Valid(saved) => {
                assert_eq!(saved.token, "abc123");
                assert_eq!(
                    saved.captured_at,
                    Some(ts.parse::<DateTime<Utc>>().unwrap())
                );
            }
            other => panic!("expected Valid, got {other:?}"),
        }

        // Legacy bare token with an unknown capture time.
        match parse_saved_token_strict("  legacy_raw\n") {
            TokenParse::Valid(saved) => {
                assert_eq!(saved.token, "legacy_raw");
                assert!(saved.captured_at.is_none());
            }
            other => panic!("expected Valid, got {other:?}"),
        }

        // Empty vs malformed are distinct so a truncated write is never a token.
        assert_eq!(parse_saved_token_strict("   \n"), TokenParse::Empty);
        assert_eq!(
            parse_saved_token_strict(r#"{"token":"abc"#),
            TokenParse::Malformed
        );
        assert_eq!(parse_saved_token_strict("{}"), TokenParse::Malformed);
        assert_eq!(
            parse_saved_token_strict(r#"{"token":"   "}"#),
            TokenParse::Malformed
        );
    }

    #[test]
    fn saved_token_debug_redacts_token() {
        let saved = SavedToken {
            token: "super-secret".to_string(),
            captured_at: None,
        };
        let rendered = format!("{saved:?}");
        assert!(
            !rendered.contains("super-secret"),
            "token leaked: {rendered}"
        );
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn unknown_capture_time_is_never_expired() {
        assert!(!is_token_expired(None));
        assert!(token_age(None).is_none());
    }

    #[test]
    fn fresh_token_is_not_expired() {
        let just_now = Some(Utc::now());
        assert!(!is_token_expired(just_now));
    }

    #[test]
    fn old_token_is_expired() {
        let old = Some(Utc::now() - Duration::hours(TOKEN_VALIDITY_HOURS + 1));
        assert!(is_token_expired(old));
    }

    #[test]
    fn boundary_token_at_window_is_expired() {
        let boundary =
            Some(Utc::now() - Duration::hours(TOKEN_VALIDITY_HOURS) - Duration::seconds(1));
        assert!(is_token_expired(boundary));
    }

    #[test]
    fn future_timestamp_is_treated_as_unknown() {
        let future = Some(Utc::now() + Duration::hours(2));
        assert!(token_age(future).is_none());
        assert!(!is_token_expired(future));
    }

    #[test]
    fn slightly_future_timestamp_clamps_to_zero() {
        let slight = Some(Utc::now() + Duration::minutes(1));
        assert_eq!(token_age(slight), Some(Duration::zero()));
    }

    #[test]
    fn expiry_message_includes_age_when_known() {
        let old = Some(Utc::now() - Duration::hours(27));
        let msg = token_expiry_message(old);
        assert!(
            msg.contains("27h ago"),
            "message should include age, got: {}",
            msg
        );
    }

    #[test]
    fn expiry_message_omits_age_when_unknown() {
        let msg = token_expiry_message(None);
        assert!(
            !msg.contains("captured"),
            "message should omit age, got: {}",
            msg
        );
    }
}

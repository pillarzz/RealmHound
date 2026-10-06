//! RotMG API client implementation.
//!
//! Provides HTTP client for calling RotMG's web API endpoints to fetch
//! character data, exaltations, and other account information.

use reqwest::blocking::Client;
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

/// Base URL for RotMG's web API (AppSpot endpoint). Returns more data than the
/// main domain, including vault contents.
const API_BASE_URL: &str = "https://realmofthemadgodhrd.appspot.com";

/// API error types.
#[derive(Error, Debug)]
pub enum ApiError {
    /// HTTP request failed
    #[error("HTTP request failed: {0}")]
    Http(&'static str),

    /// Authentication failed (invalid token)
    #[error("Authentication failed: {0}")]
    Auth(String),

    /// Rate limited by server
    #[error("Rate limited, please wait")]
    RateLimited,

    /// Server returned an error
    #[error("Server error: {0}")]
    Server(String),

    /// Invalid response from server
    #[error("Invalid response: {0}")]
    InvalidResponse(String),
}

impl From<reqwest::Error> for ApiError {
    fn from(error: reqwest::Error) -> Self {
        // Do not retain the URL, source chain, or server-supplied error text.
        Self::Http(if error.is_timeout() {
            "request timed out"
        } else if error.is_connect() {
            "connection failed"
        } else if error.is_body() || error.is_decode() {
            "response could not be read"
        } else if error.is_redirect() {
            "redirect failed"
        } else {
            "request could not be completed"
        })
    }
}

/// RotMG API client for fetching character data.
///
/// Uses an access token captured from game traffic to authenticate
/// with RotMG's web API.
#[derive(Clone)]
pub struct RotmgApiClient {
    client: Client,
    access_token: String,
    /// Debug-only diagnostics directory for raw API response snapshots. `None`
    /// (and every release build) disables snapshotting. Supplied only when a
    /// verified account profile diagnostics path exists.
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    diagnostics_dir: Option<PathBuf>,
}

impl std::fmt::Debug for RotmgApiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RotmgApiClient")
            .field("access_token", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl RotmgApiClient {
    /// Create a new API client with no diagnostics snapshotting. `access_token`
    /// is captured from a HELLO packet.
    pub fn new(access_token: String) -> Self {
        Self::with_diagnostics(access_token, None)
    }

    /// Create a new API client that, in debug builds only, snapshots raw
    /// responses under `diagnostics_dir` when it is `Some`. Release builds never
    /// write diagnostics regardless of this argument.
    pub fn with_diagnostics(access_token: String, diagnostics_dir: Option<PathBuf>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            // Force HTTP/1.1 to match the reference client's behavior
            .http1_only()
            // Add a User-Agent to look more like a normal HTTP client
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) RealmHound/1.0")
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            access_token,
            diagnostics_dir,
        }
    }

    /// Get the character list for the logged-in account.
    ///
    /// Returns the raw XML response from the server containing:
    /// - Account information
    /// - List of characters with stats, equipment, fame
    /// - PCStats (detailed statistics, encoded)
    ///
    /// # Example Response Structure
    ///
    /// ```xml
    /// <Chars>
    ///   <Account>...</Account>
    ///   <Char id="123">
    ///     <ObjectType>...</ObjectType>
    ///     <CurrentFame>...</CurrentFame>
    ///     <Equipment>...</Equipment>
    ///     <PCStats>...</PCStats>
    ///   </Char>
    /// </Chars>
    /// ```
    pub fn get_char_list(&self) -> Result<String, ApiError> {
        self.make_request("char/list")
    }

    /// Get power-up (exaltation) stats for the account.
    ///
    /// Returns XML with exaltation progress for each class.
    pub fn get_power_up_stats(&self) -> Result<String, ApiError> {
        self.make_request("account/listPowerUpStats")
    }

    /// Get the pet skins the account owns.
    ///
    /// Returns XML holding the owned pet-skin object ids, e.g.
    /// `<PetSkins>32846,50290</PetSkins>`. Pet-skin ownership is never sent
    /// over the game socket, so this endpoint is the only bulk source of it.
    pub fn get_owned_pet_skins(&self) -> Result<String, ApiError> {
        self.make_request("account/getOwnedPetSkins")
    }

    /// Get the seasonal battle-pass mission DEFINITIONS (names, descriptions,
    /// objectives, rewards) for the account's available seasons.
    ///
    /// Returns raw JSON. The raw body is written to
    /// the debug folder by [`Self::save_response_debug`].
    pub fn get_client_seasons(&self) -> Result<String, ApiError> {
        self.make_request("missions/getClientSeasons")
    }

    /// Get the player's CURRENT seasonal mission progress and claim state
    /// (server-authoritative). Returns raw XML/JSON. The raw body is written to
    /// the debug folder by
    /// [`Self::save_response_debug`].
    pub fn get_player_missions(&self) -> Result<String, ApiError> {
        self.make_request("missions/getPlayerMissions")
    }

    /// Make an authenticated request to the RotMG API.
    fn make_request(&self, endpoint: &str) -> Result<String, ApiError> {
        // Build the request params (matches the game's login request format):
        // do_login=true&accessToken=<encoded>&game_net=Unity&play_platform=Unity&game_net_user_id
        let encoded_token = urlencoding::encode(&self.access_token);

        // Random 4-digit cache buster (1000-9999) to avoid cached responses
        let ignore: u32 = 1000 + (rand::random::<u32>() % 9000);

        // Extra login parameters that make the endpoint return vault data
        let params = format!(
            "do_login=true&accessToken={}&game_net=Unity&play_platform=Unity&game_net_user_id&muleDump=true&__source=RealmHound-rust&ignore={}",
            encoded_token,
            ignore
        );

        // Params go in BOTH the URL and the body (URL = s1 + s2, body = s2)
        let url = format!("{}/{}?{}", API_BASE_URL, endpoint, params);

        tracing::debug!(
            "[API] POST {} (token len={})",
            endpoint,
            self.access_token.len()
        );

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "text/plain; charset=utf-8")
            .body(params.clone())
            .send()?;

        let status = response.status();
        let text = response.text()?;

        tracing::debug!("[API] Response: {} ({} bytes)", status.as_u16(), text.len());

        let text = Self::check_response(status, text)?;
        self.save_response_debug(endpoint, &text);
        Ok(text)
    }

    fn check_response(status: reqwest::StatusCode, text: String) -> Result<String, ApiError> {
        if status.is_success() {
            if let Some((_, after)) = text.split_once("<Error>") {
                if let Some((error_msg, _)) = after.split_once("</Error>") {
                    let lower = error_msg.to_lowercase();
                    // Map token/credential failures to Auth. An expired token
                    // reports "Account credentials not valid" (no "token"/"auth").
                    if lower.contains("token")
                        || lower.contains("auth")
                        || lower.contains("credentials not valid")
                    {
                        return Err(ApiError::Auth("Account credentials not valid".to_string()));
                    }
                    if lower.contains("account in use") {
                        return Err(ApiError::Server("Account in use".to_string()));
                    }
                }
                return Err(ApiError::Server("Request rejected by server".to_string()));
            }
            Ok(text)
        } else if status.as_u16() == 429 {
            Err(ApiError::RateLimited)
        } else if status.as_u16() == 401 || status.as_u16() == 403 {
            Err(ApiError::Auth("Account credentials not valid".to_string()))
        } else {
            Err(ApiError::Server(format!("HTTP {}", status.as_u16())))
        }
    }

    /// Save an API response snapshot for inspection.
    ///
    /// Compiled only in debug builds and writes only when a verified account
    /// profile diagnostics directory was supplied. Release builds cannot write
    /// diagnostics. The functional `char_list.xml` cache is written elsewhere
    /// (VaultPanel), not here.
    #[cfg(debug_assertions)]
    fn save_response_debug(&self, endpoint: &str, response: &str) {
        let Some(dir) = self.diagnostics_dir.as_ref() else {
            return;
        };
        if std::fs::create_dir_all(dir).is_ok() {
            let filename = format!("{}.xml", sanitize_endpoint(endpoint));
            let filepath = dir.join(filename);
            let response = if self.access_token.is_empty() {
                response.to_string()
            } else {
                response
                    .replace(&*urlencoding::encode(&self.access_token), "<redacted>")
                    .replace(&self.access_token, "<redacted>")
            };
            if let Err(e) = std::fs::write(&filepath, response) {
                tracing::warn!("[API] Failed to save debug response: {}", e);
            } else {
                tracing::debug!("[API] Saved response to {:?}", filepath);
            }
        }
    }

    /// Release builds never persist API diagnostics.
    #[cfg(not(debug_assertions))]
    fn save_response_debug(&self, _endpoint: &str, _response: &str) {}
}

/// Sanitize an endpoint into a safe flat filename stem (e.g. `char/list` ->
/// `char_list`), replacing every character that is not alphanumeric, `-`, or
/// `_` so the snapshot cannot escape the diagnostics directory.
#[cfg(debug_assertions)]
fn sanitize_endpoint(endpoint: &str) -> String {
    endpoint
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_errors_discard_url_and_source() {
        let token = "synthetic-secret";
        let error = Client::new()
            .get(format!("invalid-scheme://host/?accessToken={token}"))
            .send()
            .unwrap_err();
        let error = ApiError::from(error);
        assert!(!format!("{error} {error:?}").contains(token));
        assert!(std::error::Error::source(&error).is_none());
    }

    #[test]
    fn response_errors_never_echo_bodies() {
        for status in [200, 401, 403, 429, 500] {
            for body in [
                "<Error>synthetic-secret</Error>",
                "<Error>token synthetic-secret</Error>",
                "<Error>Account in use synthetic-secret</Error>",
                "</Error><Error>synthetic-secret",
            ] {
                let error = RotmgApiClient::check_response(
                    reqwest::StatusCode::from_u16(status).unwrap(),
                    body.to_string(),
                )
                .unwrap_err();
                assert!(!format!("{error} {error:?}").contains("synthetic-secret"));
            }
        }
    }

    #[test]
    fn account_in_use_remains_recognizable() {
        let error = RotmgApiClient::check_response(
            reqwest::StatusCode::OK,
            "<Error>Account in use</Error>".to_string(),
        )
        .unwrap_err();
        assert!(matches!(error, ApiError::Server(message) if message == "Account in use"));
    }

    #[test]
    fn client_debug_redacts_token() {
        let client = RotmgApiClient::new("synthetic-secret".to_string());
        assert!(!format!("{client:?}").contains("synthetic-secret"));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn diagnostics_are_skipped_without_a_profile_directory() {
        // Debug writes require an explicit diagnostics directory. The functional
        // char_list cache is separate.
        let temp = tempfile::tempdir().unwrap();
        let client = RotmgApiClient::with_diagnostics("token".to_string(), None);
        client.save_response_debug("char/list", "<Chars/>");
        // Nothing is created anywhere under the (empty) temp dir.
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn diagnostics_sanitize_endpoint_and_write_under_supplied_dir() {
        let temp = tempfile::tempdir().unwrap();
        let diag = temp.path().join("diagnostics").join("api");
        let client = RotmgApiClient::with_diagnostics("token".to_string(), Some(diag.clone()));
        client.save_response_debug("char/list", "<Chars/>");
        // "char/list" is sanitized to a flat, contained filename.
        assert!(diag.join("char_list.xml").exists());
        assert_eq!(sanitize_endpoint("char/list"), "char_list");
        assert_eq!(sanitize_endpoint("a/../b"), "a____b");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn diagnostics_redact_plain_and_encoded_token() {
        let temp = tempfile::tempdir().unwrap();
        let client = RotmgApiClient::with_diagnostics(
            "synthetic+secret/=".to_string(),
            Some(temp.path().to_path_buf()),
        );
        client.save_response_debug(
            "char/list",
            "<Chars>synthetic+secret/= synthetic%2Bsecret%2F%3D</Chars>",
        );
        let snapshot = std::fs::read_to_string(temp.path().join("char_list.xml")).unwrap();
        assert!(!snapshot.contains("synthetic"));
    }

    // Note: Live API tests would require a valid access token
    // and should be run manually or in integration tests
}

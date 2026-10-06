//! Parses the `season/seasonInfo` and `season/bpInfo` HTTP responses.
//!
//! The mission payload only carries the season's pool timestamp, which keeps
//! reporting the *previous* cycle's start after a rollover, so the timers get
//! their windows from these two season endpoints instead:
//!
//! ```json
//! // season/seasonInfo
//! { "name": "The Frosted Journey", "start": 1768298400, "end": 1771927140 }
//! // season/bpInfo
//! { "battlePass": { "title": "Frosted Path", "start": 1768298401, "end": 1770112740 } }
//! ```
//!
//! Both are lenient: unknown fields are ignored, missing fields default, and a
//! response without a usable window parses to `None` so callers keep their
//! estimate.

use serde::Deserialize;

/// The running season's window and name (`season/seasonInfo`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct SeasonInfo {
    /// Display name of the season, e.g. `The Frosted Journey`.
    #[serde(default)]
    pub name: String,
    /// Season start (Unix seconds). `0` when absent.
    #[serde(default)]
    pub start: i64,
    /// Season end (Unix seconds). `0` when absent.
    #[serde(default)]
    pub end: i64,
}

impl SeasonInfo {
    /// The season window when both ends are usable.
    pub fn window(&self) -> Option<(i64, i64)> {
        (self.start > 0 && self.end > self.start).then_some((self.start, self.end))
    }
}

/// Decode a `season/seasonInfo` response.
pub fn parse_season_info(body: &str) -> Option<SeasonInfo> {
    let info: SeasonInfo = serde_json::from_str(body).ok()?;
    (info.start > 0 || info.end > 0 || !info.name.is_empty()).then_some(info)
}

/// The battlepass entry inside a `season/bpInfo` response.
#[derive(Debug, Clone, Default, Deserialize)]
struct BattlePass {
    #[serde(default)]
    title: String,
    #[serde(default)]
    start: i64,
    #[serde(default)]
    end: i64,
}

/// A `season/bpInfo` response; the battlepass is absent between battlepasses.
#[derive(Debug, Clone, Default, Deserialize)]
struct BattlePassInfo {
    #[serde(default, rename = "battlePass")]
    battle_pass: Option<BattlePass>,
}

/// The running battlepass's `(title, start, end)`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BattlePassWindow {
    /// Display name of the battlepass, e.g. `Frosted Path`.
    pub title: String,
    /// Battlepass start (Unix seconds).
    pub start: i64,
    /// Battlepass end (Unix seconds).
    pub end: i64,
}

impl BattlePassWindow {
    /// The battlepass window when both ends are usable.
    pub fn window(&self) -> Option<(i64, i64)> {
        (self.start > 0 && self.end > self.start).then_some((self.start, self.end))
    }
}

/// Decode a `season/bpInfo` response's battlepass.
pub fn parse_battlepass_info(body: &str) -> Option<BattlePassWindow> {
    let info: BattlePassInfo = serde_json::from_str(body).ok()?;
    let bp = info.battle_pass?;
    Some(BattlePassWindow {
        title: bp.title.trim().to_string(),
        start: bp.start,
        end: bp.end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed real response shape from the documented payload.
    #[test]
    fn parses_the_season_window() {
        let body = r#"{
            "id": "4780423953558124",
            "name": "The Frosted Journey",
            "start": 1768298400,
            "end": 1771927140,
            "popupData": "The Frosted Journey;https://storage.googleapis.com/x.jpg"
        }"#;
        let info = parse_season_info(body).unwrap();
        assert_eq!(info.name, "The Frosted Journey");
        assert_eq!(info.window(), Some((1768298400, 1771927140)));
    }

    /// The battlepass sits nested under `battlePass`, followed by a large
    /// `milestones` array that must be ignored.
    #[test]
    fn parses_the_battlepass_window() {
        let body = r#"{
            "battlePass": {
                "id": "4604397097091072",
                "title": "Frosted Path ",
                "start": 1768298401,
                "end": 1770112740,
                "milestones": [ { "bxp": 0, "id": 1, "rewards": { "coupons": 0 } } ]
            }
        }"#;
        let bp = parse_battlepass_info(body).unwrap();
        assert_eq!(bp.title, "Frosted Path");
        assert_eq!(bp.window(), Some((1768298401, 1770112740)));
    }

    #[test]
    fn error_and_partial_bodies_yield_no_window() {
        assert!(parse_season_info("<Error>Account not Found</Error>").is_none());
        assert!(parse_battlepass_info("<Error>Account not Found</Error>").is_none());
        assert!(parse_battlepass_info("{}").is_none());
        assert!(parse_battlepass_info(r#"{"battlePass": null}"#).is_none());
        // A window needs both ends, in order.
        let half = parse_season_info(r#"{"name": "x", "start": 100}"#).unwrap();
        assert_eq!(half.window(), None);
        let backwards = parse_battlepass_info(r#"{"battlePass": {"start": 200, "end": 100}}"#);
        assert_eq!(backwards.unwrap().window(), None);
    }
}

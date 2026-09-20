//! Diagnostic CSV log of Live Feed event notifications.
//!
//! When the global opt-in is enabled, every Live Feed boss call is appended as
//! one row to the injected per-account event-notification log, including calls
//! that stayed silent. This lets us later find encounters whose notification
//! sound never plays or resolves to the wrong id (e.g. biome / "New" re-skins
//! that spawn under a different object id than the catalog representative).
//!
//! Columns: `timestamp, display_name, spawn_id, matched_id, section, icon,
//! outcome`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use realmhound_core::assets::get_asset_manager;
use realmhound_core::settings::RealmEventSounds;

use crate::realm_event_sound::{diagnose, EventSets};

const HEADER: &str = "timestamp,display_name,spawn_id,matched_id,section,icon,outcome";

fn taunt_capture_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("RealmHound").join("oryx_taunt_capture.csv"))
}

const TAUNT_HEADER: &str = "timestamp,name,raw_text";

/// Append one raw Oryx / announcement taunt packet to a discovery CSV at
/// `%LOCALAPPDATA%\RealmHound\oryx_taunt_capture.csv`. Best-effort.
///
/// The spawn detection only understands the `stringlist.<Name>.new.N` key; the
/// per-encounter death and "still alive" taunts arrive under other keys the app
/// does not yet parse. Capturing their raw form lets us build the exact
/// death-taunt mapping needed to reset the boss-call dedup on a confirmed kill
/// (so an immediate chain-respawn re-notifies). Only non-`.new.` announcements
/// are captured, to keep the file focused on the unknown keys.
pub fn capture_oryx_taunt(name: &str, raw_text: &str) {
    let Some(path) = taunt_capture_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        if fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let row = format!(
        "{},{},{}\n",
        csv_field(&timestamp),
        csv_field(name),
        csv_field(raw_text),
    );
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let mut buf = String::new();
        let is_empty = file.metadata().map(|m| m.len() == 0).unwrap_or(false);
        if is_empty {
            buf.push_str(TAUNT_HEADER);
            buf.push('\n');
        }
        buf.push_str(&row);
        let _ = file.write_all(buf.as_bytes());
    }
}

/// Escape a value for a single CSV field (quote if it contains a comma, quote,
/// or newline; double interior quotes).
fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// The object's primary sprite as `sheet:index` (e.g. `risenHellChars16x16:6`),
/// or `-` when the id is unknown / has no texture.
fn icon_ref(boss_id: i32) -> String {
    get_asset_manager()
        .get_object(boss_id)
        .and_then(|o| {
            o.primary_texture()
                .map(|t| format!("{}:{}", t.name, t.index))
        })
        .unwrap_or_else(|| "-".to_string())
}

/// Append one Live Feed boss call to the diagnostic CSV. Best-effort: any I/O
/// error is logged and swallowed so a failing log never disrupts playback.
///
/// `display_name` is the callout name as parsed by the chat layer; `boss_id` is
/// the spawn id. `should_sound` is the upstream gate result (false when the Live
/// Feed dedup or Crystal pin suppressed this callout) so the outcome column can
/// distinguish a real playback from a suppressed one. `events`/`custom_sounds`
/// describe the current configuration so the outcome column can report the
/// resolved sound and whether a custom file backs it.
pub fn log_boss_call(
    history_path: Option<&std::path::Path>,
    boss_id: i32,
    display_name: &str,
    text: &str,
    should_sound: bool,
    events: &RealmEventSounds,
    custom_sounds: &std::collections::HashMap<String, String>,
    sets: &EventSets,
) {
    // Written only when the caller supplies a per-account history path (the
    // global opt-in Settings toggle is enabled). Off by default.
    let Some(path) = history_path else {
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            tracing::warn!("event log: failed to create dir: {}", e);
            return;
        }
    }

    let diag = diagnose(events, boss_id, text, sets);
    // What the current configuration would resolve for this call, independent of
    // whether it was actually allowed to sound.
    let resolved = match (&diag.trigger, diag.silent_reason) {
        (Some(t), _) => {
            let kind = if custom_sounds.contains_key(&t.custom_key) {
                "custom"
            } else {
                "default"
            };
            format!(
                "{} sound={:?} key={} vol={:.2}",
                kind, t.sound, t.custom_key, t.volume
            )
        }
        (None, Some(reason)) => format!("SILENT: {}", reason.as_str()),
        (None, None) => "SILENT: unknown".to_string(),
    };
    // `should_sound` is false when an upstream layer (the Live Feed dedup, or the
    // Crystal pin) suppressed this callout. Record that truthfully so the log
    // isn't misread as "played" for a call that never reached the speakers.
    let outcome = if !should_sound {
        format!("SUPPRESSED: dedup (would {})", resolved)
    } else if diag.trigger.is_some() {
        format!("PLAYED {}", resolved)
    } else {
        resolved
    };

    let matched = diag
        .matched_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "-".to_string());
    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

    let row = format!(
        "{},{},{},{},{},{},{}\n",
        csv_field(&timestamp),
        csv_field(display_name),
        boss_id,
        matched,
        csv_field(diag.section),
        csv_field(&icon_ref(boss_id)),
        csv_field(&outcome),
    );

    match OpenOptions::new().create(true).append(true).open(path) {
        Ok(mut file) => {
            let mut buf = String::new();
            // Write the header only when the file is empty (covers a newly
            // created or previously-truncated zero-byte file, and avoids the
            // exists()-before-open race).
            let is_empty = file.metadata().map(|m| m.len() == 0).unwrap_or(false);
            if is_empty {
                buf.push_str(HEADER);
                buf.push('\n');
            }
            buf.push_str(&row);
            if let Err(e) = file.write_all(buf.as_bytes()) {
                tracing::warn!("event log: write failed: {}", e);
            }
        }
        Err(e) => tracing::warn!("event log: open failed: {}", e),
    }
}

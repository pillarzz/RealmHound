//! Strip the connection handshake out of `.rhcap` captures.
//!
//! `Hello` (74) carries the session ticket in its `access_token`, and `Reconnect`
//! carries the next one. The packet cipher is keyed by the packet tick rather than
//! by a secret, so a ticket inside a capture is readable by anyone holding the file.
//! The recorder drops both frames before they reach the disk nowadays; this rewrites
//! captures recorded before that fix so they can still be shared for debugging.
//!
//! The frames are located by replaying the capture through the exact TCP reassembler
//! and parser used live, so this sees the same packets the recorder does. A hit drops
//! the frame that completed the handshake plus the three before it, because a
//! handshake can span TCP segments and only the assembled packet identifies it.
//!
//! Usage:
//!   cargo run -p realmhound-core --example redact_capture -- <file-or-dir> [--dry-run]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use realmhound_core::capture::{read_capture, CaptureWriter, RawPacket};
use realmhound_core::protocol::{parse_packet, ParsedPacket};
use realmhound_core::stream::{parse_tcp_segment, TcpReassembler};

/// Frames to drop *before* a handshake frame, so a split handshake is covered.
const LOOKBACK_FRAMES: usize = 3;

/// Indexes of the frames that carry a `Hello` or `Reconnect`.
fn handshake_frames(raw: &[RawPacket]) -> Vec<usize> {
    let mut reassembler = TcpReassembler::new();
    let mut hits = Vec::new();
    for (index, rp) in raw.iter().enumerate() {
        let Some(segment) = parse_tcp_segment(rp) else {
            continue;
        };
        for packet in reassembler.process_segment(&segment) {
            if matches!(
                parse_packet(packet.packet_type(), &packet.payload),
                Some(ParsedPacket::Hello(_) | ParsedPacket::Reconnect(_))
            ) {
                hits.push(index);
            }
        }
    }
    hits
}

enum Outcome {
    /// Nothing to remove.
    Clean(usize),
    /// The handshake frames are here, but `--dry-run` was passed.
    WouldRedact(usize, usize),
    Redacted {
        kept: usize,
        dropped: usize,
        leftover: usize,
    },
    Failed(String),
}

impl Outcome {
    fn describe(&self) -> String {
        match self {
            Self::Clean(frames) => format!("no handshake ({frames} frames)"),
            Self::WouldRedact(frames, dropped) => {
                format!("would drop {dropped} of {frames} frames")
            }
            Self::Redacted {
                kept,
                dropped,
                leftover,
            } => {
                let mut line = format!("dropped {dropped} frames, kept {kept}");
                if *leftover > 0 {
                    // The stream is re-decoded against a different tick after the
                    // hole, so these are usually misparsed packets, not credentials.
                    line.push_str(&format!(
                        " ({leftover} later packet(s) still parse as a handshake)"
                    ));
                }
                line
            }
            Self::Failed(why) => format!("FAILED: {why}"),
        }
    }
}

fn redact(path: &Path, dry_run: bool) -> Outcome {
    let raw = match read_capture(path) {
        Ok(raw) => raw,
        Err(e) => return Outcome::Failed(format!("read: {e}")),
    };
    let original_frames = raw.len();
    let mut frames = raw;

    let hits = handshake_frames(&frames);
    if hits.is_empty() {
        return Outcome::Clean(original_frames);
    }
    if dry_run {
        return Outcome::WouldRedact(original_frames, hits.len());
    }
    // One pass only. Dropping frames leaves a hole in the TCP stream, and the cipher
    // is re-aligned by searching for a tick that parses, so a second scan can see
    // "handshakes" that are really packets decoded against the wrong tick -- chasing
    // those would eat session data.
    let mut drop: BTreeSet<usize> = BTreeSet::new();
    for hit in hits {
        for index in hit.saturating_sub(LOOKBACK_FRAMES)..=hit {
            drop.insert(index);
        }
    }
    let before = frames.len();
    frames = frames
        .into_iter()
        .enumerate()
        .filter(|(index, _)| !drop.contains(index))
        .map(|(_, packet)| packet)
        .collect();
    let dropped = before - frames.len();

    let tmp: PathBuf = path.with_extension("rhcap.redacted");
    let written = {
        let mut writer = match CaptureWriter::create(&tmp) {
            Ok(writer) => writer,
            Err(e) => return Outcome::Failed(format!("create temp: {e}")),
        };
        let mut error = None;
        for packet in &frames {
            if let Err(e) = writer.write(packet) {
                error = Some(format!("write: {e}"));
                break;
            }
        }
        if let Err(e) = writer.flush() {
            error = Some(format!("flush: {e}"));
        }
        match error {
            Some(why) => return Outcome::Failed(why),
            None => writer.count(),
        }
    };
    if written as usize != frames.len() {
        let _ = fs::remove_file(&tmp);
        return Outcome::Failed(format!("wrote {written} of {} frames", frames.len()));
    }
    // Read the copy back before it replaces the original: a capture that no longer
    // replays is worse than one that still holds a ticket.
    match read_capture(&tmp) {
        Ok(check) if check.len() == frames.len() => {}
        Ok(check) => {
            let _ = fs::remove_file(&tmp);
            return Outcome::Failed(format!(
                "copy holds {} frames, expected {}",
                check.len(),
                frames.len()
            ));
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            return Outcome::Failed(format!("read back: {e}"));
        }
    }
    if let Err(e) = fs::copy(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Outcome::Failed(format!("replace: {e}"));
    }
    let _ = fs::remove_file(&tmp);
    Outcome::Redacted {
        kept: frames.len(),
        dropped,
        leftover: handshake_frames(&frames).len(),
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(target) = args.next() else {
        eprintln!("usage: redact_capture <file-or-dir> [--dry-run]");
        std::process::exit(2);
    };
    let dry_run = args.any(|a| a == "--dry-run");
    let target = PathBuf::from(target);

    let mut paths: Vec<PathBuf> = if target.is_dir() {
        match fs::read_dir(&target) {
            Ok(entries) => entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e == "rhcap"))
                .collect(),
            Err(e) => {
                eprintln!("failed to list {}: {e}", target.display());
                std::process::exit(1);
            }
        }
    } else {
        vec![target.clone()]
    };
    paths.sort();

    let (mut redacted, mut clean, mut failed) = (0usize, 0usize, 0usize);
    for path in &paths {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let outcome = redact(path, dry_run);
        match &outcome {
            Outcome::Redacted { .. } => redacted += 1,
            Outcome::WouldRedact(..) => redacted += 1,
            Outcome::Clean(_) => clean += 1,
            Outcome::Failed(_) => failed += 1,
        }
        println!("{name}: {}", outcome.describe());
    }
    println!(
        "\n{} file(s): {redacted} {}, {clean} clean, {failed} failed",
        paths.len(),
        if dry_run {
            "need redacting"
        } else {
            "redacted"
        }
    );
    if failed > 0 {
        std::process::exit(1);
    }
}

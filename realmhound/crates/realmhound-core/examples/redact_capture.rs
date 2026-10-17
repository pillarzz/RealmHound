//! Strip the session ticket out of old `.rhcap` captures.
//!
//! The client's `Hello` (74) carries the ticket in its `access_token`. The packet
//! cipher is keyed by the packet tick rather than by a secret, so a ticket inside a
//! capture is readable by anyone holding the file. The recorder drops that frame
//! before it reaches the disk nowadays; this rewrites captures recorded before that
//! fix so they can still be shared for debugging.
//!
//! Only frames the client sent are candidates: the server's `Hello` reply and its
//! `Reconnect` carry the map key and map id, and the `MapInfo` frames that follow
//! carry the map seed all the damage and replay maths is built on, so none of them
//! are touched.
//!
//! Frames are classified by the same TCP reassembler and byte-range checks used
//! by the live recorder, including split handshakes and retransmissions.
//!
//! Usage:
//!   cargo run -p realmhound-core --example redact_capture -- <file-or-dir> [--dry-run]

use std::fs;
use std::path::{Path, PathBuf};

use realmhound_core::capture::{read_capture, CaptureWriter, RawPacket};
use realmhound_core::protocol::PacketType;
use realmhound_core::stream::{parse_tcp_segment, TcpReassembler};

fn handshake_count(raw: &[RawPacket]) -> usize {
    let mut reassembler = TcpReassembler::new();
    let mut hits = 0;
    for rp in raw {
        let Some(segment) = parse_tcp_segment(rp) else {
            continue;
        };
        for packet in reassembler.process_segment(&segment) {
            if !packet.incoming && packet.packet_type() == PacketType::Hello {
                hits += 1;
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
    },
    Failed(String),
}

impl Outcome {
    fn describe(&self) -> String {
        match self {
            Self::Clean(frames) => format!("no handshake ({frames} frames)"),
            Self::WouldRedact(frames, handshakes) => {
                format!("found {handshakes} client handshake(s) in {frames} frames; would redact")
            }
            Self::Redacted { kept, dropped } => format!("dropped {dropped} frames, kept {kept}"),
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
    let hits = handshake_count(&raw);
    if hits == 0 {
        return Outcome::Clean(original_frames);
    }
    if dry_run {
        return Outcome::WouldRedact(original_frames, hits);
    }

    let tmp: PathBuf = path.with_extension("rhcap.redacted");
    let written = match write_redacted_capture(&tmp, &raw) {
        Ok(written) => written as usize,
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            return Outcome::Failed(format!("write redacted copy: {e}"));
        }
    };
    // Read the copy back before it replaces the original: a capture that no longer
    // replays is worse than one that still holds a ticket.
    match read_capture(&tmp) {
        Ok(check) if check.len() == written => {}
        Ok(check) => {
            let _ = fs::remove_file(&tmp);
            return Outcome::Failed(format!(
                "copy holds {} frames, expected {}",
                check.len(),
                written
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
        kept: written,
        dropped: original_frames - written,
    }
}

fn write_redacted_capture(path: &Path, raw: &[RawPacket]) -> std::io::Result<u64> {
    let mut writer = CaptureWriter::create(path)?;
    let mut reassembler = TcpReassembler::new();
    for frame in raw {
        writer.write(frame)?;
        if let Some(segment) = parse_tcp_segment(frame) {
            for packet in reassembler.process_segment(&segment) {
                writer.classify_packet(&packet)?;
            }
        }
    }
    writer.finish()?;
    Ok(writer.count())
}

#[cfg(test)]
mod tests {
    use super::*;
    use realmhound_core::capture::PacketFormat;
    use realmhound_core::crypto::{RC4Cipher, OUTGOING_KEY};

    fn frame(sequence: u32, payload: &[u8], syn: bool, incoming: bool) -> RawPacket {
        let (src, sport, dst, dport) = if incoming {
            ([198, 51, 100, 2], 2050, [192, 0, 2, 1], 45678)
        } else {
            ([192, 0, 2, 1], 45678, [198, 51, 100, 2], 2050)
        };
        let builder =
            etherparse::PacketBuilder::ipv4(src, dst, 64).tcp(sport, dport, sequence, 32768);
        let builder = if syn {
            builder.syn()
        } else {
            builder.ack(1).psh()
        };
        let mut data = Vec::new();
        builder.write(&mut data, payload).unwrap();
        RawPacket {
            timestamp: chrono::Utc::now(),
            packet_format: PacketFormat::RawIp,
            #[cfg(feature = "latency-diagnostics")]
            enqueued_at: None,
            data,
        }
    }

    fn sample_capture() -> (Vec<RawPacket>, Vec<RawPacket>) {
        let mut cipher = RC4Cipher::new(OUTGOING_KEY);
        let mut hello = 85u32.to_be_bytes().to_vec();
        hello.push(PacketType::Hello as u8);
        hello.extend(cipher.apply_to_vec(&[42; 80]));
        let mut safe = 8u32.to_be_bytes().to_vec();
        safe.push(PacketType::PlayerText as u8);
        safe.extend(cipher.apply_to_vec(b"\0\x01x"));
        let start = frame(0, &[], true, false);
        let server = frame(1, &[0, 0, 0, 5, 8], false, true);
        let after = frame(1 + hello.len() as u32, &safe, false, false);
        let mut raw = vec![start.clone()];
        for (index, chunk) in hello.chunks(10).enumerate() {
            raw.push(frame(1 + index as u32 * 10, chunk, false, false));
        }
        raw.push(server.clone());
        raw.push(raw[1].clone());
        raw.push(after.clone());
        raw.push(after.clone());
        (raw, vec![start, server, after.clone(), after])
    }

    fn write_legacy_capture(path: &Path, frames: &[RawPacket]) {
        let mut bytes = b"RHCAP\x02".to_vec();
        for packet in frames {
            bytes.extend_from_slice(
                &packet
                    .timestamp
                    .timestamp_nanos_opt()
                    .unwrap()
                    .to_le_bytes(),
            );
            bytes.push(packet.packet_format as u8);
            bytes.extend_from_slice(&(packet.data.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&packet.data);
        }
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn redacts_split_and_retransmitted_hellos_without_losing_safe_outgoing_frames() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.rhcap");
        let (raw, expected) = sample_capture();
        write_legacy_capture(&path, &raw);

        let result = redact(&path, false);

        assert!(
            matches!(result, Outcome::Redacted { kept: 4, dropped } if dropped == raw.len() - 4),
            "{}",
            result.describe()
        );
        let actual = read_capture(&path).unwrap();
        assert_eq!(
            actual
                .iter()
                .map(|p| (&p.data, p.timestamp))
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|p| (&p.data, p.timestamp))
                .collect::<Vec<_>>()
        );
        assert!(!path.with_extension("rhcap.redacted").exists());
    }

    #[test]
    fn dry_run_does_not_modify_the_capture() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.rhcap");
        let (raw, _) = sample_capture();
        write_legacy_capture(&path, &raw);
        let before = fs::read(&path).unwrap();

        assert!(matches!(redact(&path, true), Outcome::WouldRedact(_, 1)));
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(!path.with_extension("rhcap.redacted").exists());
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

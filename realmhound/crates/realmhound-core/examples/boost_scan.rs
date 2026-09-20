//! Trace packets around the moment an account-wide booster potion was consumed.
//!
//! We can't see the boost as a per-object stat, so instead we window the packet
//! stream around the estimated consume time and look for the client `UseItem`
//! plus any rare / unmodeled ("Unknown") server packets clustered right after it
//! -- the likely boost-grant / confirm channel.
//!
//! Usage:
//!   cargo run -p realmhound-core --example boost_scan -- <capture.rhcap> <target_rfc3339> [radius_secs]
//!
//! The radius defaults to 720s (12 min).

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};

use realmhound_core::capture::read_capture;
use realmhound_core::stream::{parse_tcp_segment, TcpReassembler};

fn main() {
    let mut args = std::env::args().skip(1);
    const USAGE: &str = "usage: boost_scan <capture.rhcap> <target_rfc3339> [radius_secs]";
    let path = PathBuf::from(args.next().expect(USAGE));
    let target: DateTime<Utc> = DateTime::parse_from_rfc3339(&args.next().expect(USAGE))
        .expect("target_rfc3339 must be a valid RFC3339 timestamp")
        .with_timezone(&Utc);
    let radius: i64 = args
        .next()
        .map(|value| value.parse().expect("radius_secs must be an integer"))
        .unwrap_or(720);

    let raw = read_capture(&path).expect("read capture");
    let mut reassembler = TcpReassembler::new();

    // Global stats.
    let mut counts: BTreeMap<u8, (String, u64)> = BTreeMap::new();
    let mut span_min: Option<DateTime<Utc>> = None;
    let mut span_max: Option<DateTime<Utc>> = None;

    // Window rows: (time, dir, raw_id, name, size, payload preview).
    struct Row {
        t: DateTime<Utc>,
        incoming: bool,
        raw_id: u8,
        name: &'static str,
        size: usize,
        preview: String,
    }
    let mut rows: Vec<Row> = Vec::new();

    // Every id=153 packet (full payload) + any packet whose payload contains the
    // dust-potion object type bytes 0x02 0xEF, wherever it appears in the capture.
    let mut id153: Vec<(DateTime<Utc>, bool, String)> = Vec::new();
    let mut contains_2ef: Vec<(DateTime<Utc>, bool, u8, &'static str, String)> = Vec::new();
    // Every id=165 packet, decoded as ASCII (printable) + hex.
    let mut id165: Vec<(DateTime<Utc>, bool, String, String)> = Vec::new();

    let lo = target - chrono::Duration::seconds(radius);
    let hi = target + chrono::Duration::seconds(radius);

    for rp in &raw {
        let Some(segment) = parse_tcp_segment(rp) else {
            continue;
        };
        for packet in reassembler.process_segment(&segment) {
            let t = packet.timestamp;
            span_min = Some(span_min.map_or(t, |m| m.min(t)));
            span_max = Some(span_max.map_or(t, |m| m.max(t)));

            let id = packet.header.raw_id;
            let name = packet.type_name();
            let e = counts.entry(id).or_insert((name.to_string(), 0));
            e.1 += 1;

            let full_hex: String = packet
                .payload
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(" ");

            if id == 153 {
                id153.push((t, packet.incoming, full_hex.clone()));
            }
            if id == 165 {
                let ascii: String = packet
                    .payload
                    .iter()
                    .map(|&b| {
                        if (0x20..0x7f).contains(&b) {
                            b as char
                        } else {
                            '.'
                        }
                    })
                    .collect();
                id165.push((t, packet.incoming, ascii, full_hex.clone()));
            }
            if packet.payload.windows(2).any(|w| w == [0x02, 0xef]) {
                contains_2ef.push((t, packet.incoming, id, name, full_hex.clone()));
            }

            if t >= lo && t <= hi {
                let preview: String = packet
                    .payload
                    .iter()
                    .take(28)
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                rows.push(Row {
                    t,
                    incoming: packet.incoming,
                    raw_id: id,
                    name,
                    size: packet.payload.len(),
                    preview,
                });
            }
        }
    }

    println!(
        "capture span: {} .. {}  ({} raw packets)",
        span_min.map(|t| t.to_rfc3339()).unwrap_or_default(),
        span_max.map(|t| t.to_rfc3339()).unwrap_or_default(),
        raw.len()
    );
    println!(
        "target consume ~ {}  window +/-{}s",
        target.to_rfc3339(),
        radius
    );

    // Rare packet types across the whole capture (candidates for a one-shot grant).
    println!("\n=== RARE packet types (total count <= 30) ===");
    for (id, (name, n)) in &counts {
        if *n <= 30 {
            println!("  raw_id={id:>3} {name:<28} total={n}");
        }
    }

    println!("\n=== packets in window ({} rows) ===", rows.len());
    for r in &rows {
        let dir = if r.incoming { "S>C" } else { "C>S" };
        let total = counts.get(&r.raw_id).map(|(_, n)| *n).unwrap_or(0);
        let mut flags = String::new();
        if r.name == "UNKNOWN" {
            flags.push_str(" <UNKNOWN");
        }
        if r.name == "UseItem" {
            flags.push_str(" <USE-ITEM");
        }
        if total <= 30 {
            flags.push_str(" <RARE");
        }
        println!(
            "{} {dir} id={:>3} {:<26} sz={:<4}{}  [{}]",
            r.t.format("%H:%M:%S%.3f"),
            r.raw_id,
            r.name,
            r.size,
            flags,
            r.preview
        );
    }

    println!("\n=== ALL id=153 packets (full payload) ===");
    for (t, inc, hex) in &id153 {
        let dir = if *inc { "S>C" } else { "C>S" };
        println!("{} {dir}  [{}]", t.format("%H:%M:%S%.3f"), hex);
    }

    println!("\n=== payloads containing bytes 02 ef (dust type 0x2ef=751) ===");
    for (t, inc, id, name, hex) in &contains_2ef {
        let dir = if *inc { "S>C" } else { "C>S" };
        println!(
            "{} {dir} id={:>3} {:<26} [{}]",
            t.format("%H:%M:%S%.3f"),
            id,
            name,
            hex
        );
    }

    println!("\n=== ALL id=165 packets (ASCII / hex) ===");
    for (t, inc, ascii, hex) in &id165 {
        let dir = if *inc { "S>C" } else { "C>S" };
        println!(
            "{} {dir}  \"{}\"   [{}]",
            t.format("%H:%M:%S%.3f"),
            ascii,
            hex
        );
    }
}

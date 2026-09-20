//! Scan `.rhcap` captures for Crucible data: CrucibleResponse JSON payloads
//! (the Nexus Crucible-NPC UI) and any CRUCIBLE (128) stat string on objects.
//!
//! Usage:
//!   cargo run -p realmhound-core --example crucible_scan -- <path-to.rhcap> [more.rhcap ...]

use std::collections::BTreeSet;
use std::path::PathBuf;

use realmhound_core::capture::read_capture;
use realmhound_core::protocol::data::StatType;
use realmhound_core::protocol::{parse_packet, ParsedPacket};
use realmhound_core::stream::{parse_tcp_segment, TcpReassembler};

fn scan_stats(
    object_type: u16,
    object_id: i32,
    stats: &[realmhound_core::protocol::data::StatData],
    seen: &mut BTreeSet<String>,
) {
    for s in stats {
        if s.stat_type == StatType::Crucible {
            if let Some(v) = &s.string_stat_value {
                let line = format!(
                    "CRUCIBLE stat  obj_type=0x{:x} obj_id={}  value={:?}",
                    object_type, object_id, v
                );
                if seen.insert(line.clone()) {
                    println!("{line}");
                }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: crucible_scan <path-to.rhcap> [more.rhcap ...]");
        std::process::exit(2);
    }

    let mut crucible_responses = 0u64;
    let mut crucible_stat_hits = 0u64;
    let mut seen_stat_lines: BTreeSet<String> = BTreeSet::new();
    let mut seen_jsons: BTreeSet<String> = BTreeSet::new();

    for arg in &args {
        let path = PathBuf::from(arg);
        let raw = match read_capture(&path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skip {}: {}", path.display(), e);
                continue;
            }
        };
        let mut reassembler = TcpReassembler::new();
        for rp in &raw {
            let Some(segment) = parse_tcp_segment(rp) else {
                continue;
            };
            for packet in reassembler.process_segment(&segment) {
                match parse_packet(packet.packet_type(), &packet.payload) {
                    Some(ParsedPacket::CrucibleResponse(p)) => {
                        crucible_responses += 1;
                        println!(
                            "\n=== CrucibleResponse in {} ===",
                            path.file_name().unwrap().to_string_lossy()
                        );
                        for (id, json) in p.crucible_ids.iter().zip(p.crucible_jsons.iter()) {
                            println!("id={id}");
                            if seen_jsons.insert(json.clone()) {
                                println!("{json}");
                            } else {
                                println!("(json seen before)");
                            }
                        }
                    }
                    Some(ParsedPacket::Update(u)) => {
                        for obj in &u.new_objects {
                            let before = seen_stat_lines.len();
                            scan_stats(
                                obj.object_type,
                                obj.status.object_id,
                                &obj.status.stats,
                                &mut seen_stat_lines,
                            );
                            crucible_stat_hits += (seen_stat_lines.len() - before) as u64;
                        }
                    }
                    Some(ParsedPacket::NewTick(t)) => {
                        for st in &t.statuses {
                            let before = seen_stat_lines.len();
                            scan_stats(0, st.object_id, &st.stats, &mut seen_stat_lines);
                            crucible_stat_hits += (seen_stat_lines.len() - before) as u64;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    println!("\n=== Summary ===");
    println!("CrucibleResponse packets: {crucible_responses}");
    println!("Unique CRUCIBLE stat strings: {crucible_stat_hits}");
    println!("Unique crucible JSON payloads: {}", seen_jsons.len());
}

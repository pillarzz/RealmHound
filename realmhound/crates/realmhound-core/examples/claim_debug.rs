//! Dump raw packet 163 (ClaimMission) and 164 (claim ack) from a .rhcap
//! capture to diagnose mission-claiming failures.
//!
//! Usage:
//!   cargo run -p realmhound-core --example claim_debug -- <path.rhcap>

use std::path::PathBuf;

use realmhound_core::capture::read_capture;
use realmhound_core::protocol::{parse_packet, ParsedPacket};
use realmhound_core::stream::{parse_tcp_segment, TcpReassembler};

fn main() {
    let Some(arg) = std::env::args().nth(1) else {
        eprintln!("usage: claim_debug <path.rhcap>");
        std::process::exit(2);
    };
    let path = PathBuf::from(arg);

    let raw = match read_capture(&path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("failed to read {}: {}", path.display(), e);
            std::process::exit(1);
        }
    };
    println!("Read {} raw packets from {}\n", raw.len(), path.display());

    let mut reassembler = TcpReassembler::new();

    for rp in &raw {
        let Some(segment) = parse_tcp_segment(rp) else {
            continue;
        };
        for packet in reassembler.process_segment(&segment) {
            let ptype = packet.packet_type();
            let payload = &packet.payload;

            // Packet 163 = ClaimMission (outgoing)
            if ptype as u16 == 163 {
                println!("=== Packet 163 (ClaimMission) ===");
                println!("  Payload length: {} bytes", payload.len());
                println!(
                    "  Raw hex: {}",
                    payload
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
                if let Some(ParsedPacket::ClaimMission(p)) = parse_packet(ptype, payload) {
                    println!(
                        "  Parsed: season_id={}, mission_idx={}, request_id={}, mask={}",
                        p.season_id, p.mission_positional_idx, p.request_id, p.mask
                    );
                } else {
                    println!("  PARSE FAILED");
                }
                println!();
            }

            // Packet 164 = Unknown164 (incoming ack)
            if ptype as u16 == 164 {
                println!("=== Packet 164 (ClaimAck) ===");
                println!("  Payload length: {} bytes", payload.len());
                println!(
                    "  Raw hex: {}",
                    payload
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
                if let Some(ParsedPacket::Unknown164(p)) = parse_packet(ptype, payload) {
                    println!(
                        "  Parsed: byte1={}, byte2={}, short={}",
                        p.unknown_byte1, p.unknown_byte2, p.unknown_short
                    );
                    println!(
                        "  Interpretation: request_id={}, success={}",
                        p.unknown_byte1,
                        p.unknown_byte2 != 0
                    );
                } else {
                    println!("  PARSE FAILED");
                }
                println!();
            }
        }
    }
}

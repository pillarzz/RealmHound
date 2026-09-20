//! Offline replay + analysis for `.rhcap` captures recorded by RealmHound.
//!
//! Feeds a recorded raw-packet stream back through the exact TCP reassembler
//! and packet parser used live, then prints a packet-type histogram and a
//! focused breakdown of combat-relevant packets. This is the analysis tool for
//! the Combat History feasibility spike: it answers "what combat data is
//! actually observable from a single client's traffic?".
//!
//! Usage:
//!   cargo run -p realmhound-core --example replay_capture -- <path-to.rhcap>

use std::collections::BTreeMap;
use std::path::PathBuf;

use realmhound_core::capture::read_capture;
use realmhound_core::protocol::{parse_packet, ParsedPacket};
use realmhound_core::stream::{parse_tcp_segment, TcpReassembler};

fn main() {
    let Some(arg) = std::env::args().nth(1) else {
        eprintln!("usage: replay_capture <path-to.rhcap>");
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
    println!("Read {} raw packets from {}", raw.len(), path.display());

    let mut reassembler = TcpReassembler::new();
    let mut total = 0u64;
    let mut incoming = 0u64;
    let mut outgoing = 0u64;
    let mut by_type: BTreeMap<&'static str, u64> = BTreeMap::new();

    // Combat-relevant tallies for the observability matrix.
    let mut enemy_hit = 0u64;
    let mut player_hit = 0u64;
    let mut damage = 0u64;
    let mut damage_with_amount = 0u64;
    let mut deaths = 0u64;
    let mut enemy_shoot = 0u64;
    let mut server_player_shoot = 0u64;
    let mut ally_shoot = 0u64;
    let mut aoe = 0u64;
    let mut updates = 0u64;
    let mut new_ticks = 0u64;

    for rp in &raw {
        let Some(segment) = parse_tcp_segment(rp) else {
            continue;
        };
        for packet in reassembler.process_segment(&segment) {
            total += 1;
            if packet.incoming {
                incoming += 1;
            } else {
                outgoing += 1;
            }
            *by_type.entry(packet.type_name()).or_default() += 1;

            match parse_packet(packet.packet_type(), &packet.payload) {
                Some(ParsedPacket::EnemyHit(_)) => enemy_hit += 1,
                Some(ParsedPacket::PlayerHit(_)) => player_hit += 1,
                Some(ParsedPacket::Damage(d)) => {
                    damage += 1;
                    if d.damage_amount > 0 {
                        damage_with_amount += 1;
                    }
                }
                Some(ParsedPacket::Death(_)) => deaths += 1,
                Some(ParsedPacket::EnemyShoot(_)) => enemy_shoot += 1,
                Some(ParsedPacket::ServerPlayerShoot(_)) => server_player_shoot += 1,
                Some(ParsedPacket::AllyShoot(_)) => ally_shoot += 1,
                Some(ParsedPacket::Aoe(_)) => aoe += 1,
                Some(ParsedPacket::Update(_)) => updates += 1,
                Some(ParsedPacket::NewTick(_)) => new_ticks += 1,
                _ => {}
            }
        }
    }

    println!("\n=== Decoded packets ===");
    println!("total={total}  incoming(S>C)={incoming}  outgoing(C>S)={outgoing}");

    println!("\n=== Packet type histogram (by count) ===");
    let mut sorted: Vec<_> = by_type.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    for (name, count) in sorted {
        println!("{count:>8}  {name}");
    }

    println!("\n=== Combat-relevant tallies ===");
    println!("EnemyHit           {enemy_hit}");
    println!("PlayerHit          {player_hit}");
    println!("Damage             {damage} (with amount>0: {damage_with_amount})");
    println!("Death              {deaths}");
    println!("EnemyShoot         {enemy_shoot}");
    println!("ServerPlayerShoot  {server_player_shoot}");
    println!("AllyShoot          {ally_shoot}");
    println!("Aoe                {aoe}");
    println!("Update             {updates}");
    println!("NewTick            {new_ticks}");
}

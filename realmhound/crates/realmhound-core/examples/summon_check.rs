//! Ninja Sprite World boss damage breakdown with timestamps.
use std::collections::HashMap;
use std::path::PathBuf;

use realmhound_core::capture::read_capture;
use realmhound_core::protocol::{parse_packet, ParsedPacket};
use realmhound_core::stream::{parse_tcp_segment, TcpReassembler};

struct MapStats {
    map_name: String,
    local_id: i32,
    timestamp: String,
    item_uses: HashMap<i32, u32>,
    // Per-target damage from DamagePackets (local player only)
    dmg_pkt_by_target: HashMap<i32, (i64, u32)>, // target -> (total_dmg, hits)
    // EnemyHit by target (weapon hits)
    eh_by_target: HashMap<i32, u32>,
    // SPS damage (ability projectiles)
    sps_total: i64,
    sps_by_container: HashMap<i32, (i64, u32)>, // container_type -> (total_dmg, count)
}

fn main() {
    let Some(arg) = std::env::args().nth(1) else {
        eprintln!("usage: summon_check <path.rhcap>");
        std::process::exit(2);
    };
    let path = PathBuf::from(arg);
    let raw = read_capture(&path).expect("read capture");

    let mut reassembler = TcpReassembler::new();
    let mut maps: Vec<MapStats> = Vec::new();
    let mut current: Option<MapStats> = None;
    let mut last_ts = String::new();

    for raw_pkt in &raw {
        let ts = raw_pkt.timestamp.format("%H:%M:%S").to_string();
        last_ts = ts.clone();

        let segments = parse_tcp_segment(raw_pkt);
        for seg in segments {
            for packet in reassembler.process_segment(&seg) {
                let parsed = parse_packet(packet.packet_type(), &packet.payload);
                match parsed {
                    Some(ParsedPacket::MapInfo(mi)) => {
                        if let Some(m) = current.take() {
                            if m.map_name.contains("Sprite World") {
                                maps.push(m);
                            }
                        }
                        current = Some(MapStats {
                            map_name: mi.name.clone(),
                            local_id: 0,
                            timestamp: last_ts.clone(),
                            item_uses: HashMap::new(),
                            dmg_pkt_by_target: HashMap::new(),
                            eh_by_target: HashMap::new(),
                            sps_total: 0,
                            sps_by_container: HashMap::new(),
                        });
                    }
                    Some(ParsedPacket::CreateSuccess(cs)) => {
                        if let Some(ref mut m) = current {
                            m.local_id = cs.object_id;
                        }
                    }
                    Some(ParsedPacket::UseItem(ui)) => {
                        if let Some(ref mut m) = current {
                            if m.map_name.contains("Sprite World") {
                                *m.item_uses.entry(ui.slot_object.item_type).or_default() += 1;
                            }
                        }
                    }
                    Some(ParsedPacket::Damage(d)) => {
                        if let Some(ref mut m) = current {
                            if m.map_name.contains("Sprite World") && d.object_id == m.local_id {
                                let e = m.dmg_pkt_by_target.entry(d.target_id).or_default();
                                e.0 += d.damage_amount as i64;
                                e.1 += 1;
                            }
                        }
                    }
                    Some(ParsedPacket::EnemyHit(eh)) => {
                        if let Some(ref mut m) = current {
                            if m.map_name.contains("Sprite World") && eh.main_id == m.local_id {
                                *m.eh_by_target.entry(eh.target_id).or_default() += 1;
                            }
                        }
                    }
                    Some(ParsedPacket::ServerPlayerShoot(sps)) => {
                        if let Some(ref mut m) = current {
                            if m.map_name.contains("Sprite World") && sps.owner_id == m.local_id {
                                m.sps_total += sps.damage as i64;
                                let e = m.sps_by_container.entry(sps.container_type).or_default();
                                e.0 += sps.damage as i64;
                                e.1 += 1;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    if let Some(m) = current.take() {
        if m.map_name.contains("Sprite World") {
            maps.push(m);
        }
    }

    for (i, m) in maps.iter().enumerate() {
        println!(
            "\n===== Run {} | {} | {} | local_id={} =====",
            i + 1,
            m.timestamp,
            m.map_name,
            m.local_id
        );

        // Items used
        if m.item_uses.is_empty() {
            println!("  Ability: NONE USED");
        } else {
            for (item, count) in &m.item_uses {
                println!("  Ability: item 0x{:x} x{}", item, count);
            }
        }

        // Boss = the target with most total interactions (highest hit count)
        let mut all_targets: HashMap<i32, (i64, u32, u32)> = HashMap::new(); // target -> (dmg_pkt_dmg, dmg_pkt_hits, eh_hits)
        for (&t, &(dmg, hits)) in &m.dmg_pkt_by_target {
            let e = all_targets.entry(t).or_default();
            e.0 += dmg;
            e.1 += hits;
        }
        for (&t, &hits) in &m.eh_by_target {
            all_targets.entry(t).or_default().2 += hits;
        }

        let mut targets: Vec<_> = all_targets.into_iter().collect();
        targets.sort_by(|a, b| (b.1 .1 + b.1 .2).cmp(&(a.1 .1 + a.1 .2)));

        // Only show the boss (top target)
        if let Some((boss_id, (ability_dmg, ability_hits, weapon_hits))) = targets.first() {
            println!(
                "  BOSS (target={}): ability_aoe={} dmg ({} hits) | weapon={} hits",
                boss_id, ability_dmg, ability_hits, weapon_hits
            );
        }

        // SPS breakdown
        if m.sps_total > 0 {
            println!("  --- SPS (ability projectiles) ---");
            for (container, (dmg, count)) in &m.sps_by_container {
                println!(
                    "  container=0x{:x}: total_dmg={} ({} shots)",
                    container, dmg, count
                );
            }
        }
    }
}

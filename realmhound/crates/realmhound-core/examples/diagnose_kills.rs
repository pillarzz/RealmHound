//! Diagnose boss kill-detection: for each boss-like object that took damage,
//! print its HP timeline endpoints and how the tracker would decide "killed".
//!
//! Usage:
//!   cargo run -p realmhound-core --example diagnose_kills -- <path-to.rhcap>

use std::collections::HashMap;
use std::path::PathBuf;

use realmhound_core::assets::get_asset_manager;
use realmhound_core::capture::read_capture;
use realmhound_core::protocol::data::{ObjectStatusData, StatType};
use realmhound_core::protocol::{parse_packet, ParsedPacket};
use realmhound_core::stream::{parse_tcp_segment, TcpReassembler};

#[derive(Default, Clone)]
struct Obj {
    object_type: u16,
    max_hp: i64,
    first_hp: Option<i64>,
    last_hp: i64,
    min_hp: i64,
    hp_samples: u64,
    took_damage: bool,
    removed: bool,
    // last few HP values before it stopped updating / was removed
    tail: Vec<i64>,
}

fn read_hp(status: &ObjectStatusData) -> (Option<i64>, Option<i64>) {
    let mut hp = None;
    let mut max = None;
    for s in &status.stats {
        if s.stat_type_id == StatType::HP as u8 {
            hp = Some(s.stat_value as i64);
        }
        if s.stat_type_id == StatType::MaxHP as u8 {
            max = Some(s.stat_value as i64);
        }
    }
    (hp, max)
}

fn main() {
    let Some(arg) = std::env::args().nth(1) else {
        eprintln!("usage: diagnose_kills <path-to.rhcap>");
        std::process::exit(2);
    };
    let path = PathBuf::from(arg);
    let raw = read_capture(&path).expect("read capture");

    let assets = get_asset_manager();
    let dir = realmhound_core::assets::default_assets_dir();
    if dir.exists() {
        assets.set_assets_dir(&dir);
    }

    let mut objs: HashMap<i32, Obj> = HashMap::new();
    let mut reassembler = TcpReassembler::new();

    let mut note = |objs: &mut HashMap<i32, Obj>, st: &ObjectStatusData, otype: Option<u16>| {
        let e = objs.entry(st.object_id).or_default();
        if let Some(t) = otype {
            e.object_type = t;
        }
        let (hp, max) = read_hp(st);
        if let Some(m) = max {
            e.max_hp = m;
        }
        if let Some(h) = hp {
            if e.first_hp.is_none() {
                e.first_hp = Some(h);
                e.min_hp = h;
            }
            e.last_hp = h;
            e.min_hp = e.min_hp.min(h);
            e.hp_samples += 1;
            e.tail.push(h);
            if e.tail.len() > 8 {
                e.tail.remove(0);
            }
        }
    };

    for rp in &raw {
        let Some(seg) = parse_tcp_segment(rp) else {
            continue;
        };
        for packet in reassembler.process_segment(&seg) {
            match parse_packet(packet.packet_type(), &packet.payload) {
                Some(ParsedPacket::Update(u)) => {
                    for obj in &u.new_objects {
                        note(&mut objs, &obj.status, Some(obj.object_type));
                    }
                    for &rid in &u.drops {
                        if let Some(e) = objs.get_mut(&rid) {
                            e.removed = true;
                        }
                    }
                }
                Some(ParsedPacket::NewTick(t)) => {
                    for st in &t.statuses {
                        note(&mut objs, st, None);
                    }
                }
                Some(ParsedPacket::Damage(d)) => {
                    if let Some(e) = objs.get_mut(&d.target_id) {
                        e.took_damage = true;
                    } else {
                        objs.entry(d.target_id).or_default().took_damage = true;
                    }
                }
                _ => {}
            }
        }
    }

    let mut bosses: Vec<(&i32, &Obj)> = objs
        .iter()
        .filter(|(_, o)| {
            o.took_damage
                && o.object_type != 0
                && (assets.is_boss_like(o.object_type as i32) || o.max_hp >= 10_000)
        })
        .collect();
    bosses.sort_by_key(|(_, o)| std::cmp::Reverse(o.max_hp));

    println!("id       name                              maxhp    first   last    min    %dropped removed tail");
    for (id, o) in bosses {
        let name = assets
            .object_name(o.object_type as i32)
            .unwrap_or_else(|| format!("type#{}", o.object_type));
        let first = o.first_hp.unwrap_or(0);
        let pct_dropped = if o.max_hp > 0 {
            100.0 * (o.max_hp - o.last_hp) as f64 / o.max_hp as f64
        } else {
            0.0
        };
        println!(
            "{:<8} {:<33} {:<8} {:<7} {:<7} {:<6} {:>6.1}%  {:<6} {:?}",
            id, name, o.max_hp, first, o.last_hp, o.min_hp, pct_dropped, o.removed, o.tail
        );
    }
}

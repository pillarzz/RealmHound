//! Diagnose boss DETECTION: list every enemy object that took damage, with its
//! catalog labels, max HP, HP drop, and whether the current classifier would
//! treat it as its own fight. Helps find under-detected minibosses (labeled
//! MINION but real, e.g. deep-sea gods) and over-detected clutter.
//!
//! Usage:
//!   cargo run -p realmhound-core --example diagnose_detect -- <path-to.rhcap>

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
    took_damage: bool,
    removed: bool,
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
        eprintln!("usage: diagnose_detect <path-to.rhcap>");
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
                    objs.entry(d.target_id).or_default().took_damage = true;
                }
                _ => {}
            }
        }
    }

    let mut list: Vec<(&i32, &Obj)> = objs
        .iter()
        .filter(|(_, o)| {
            o.object_type != 0
                && (o.took_damage
                    || (o.first_hp.is_some() && o.max_hp > 0 && o.min_hp < o.max_hp * 90 / 100))
        })
        .collect();
    list.sort_by_key(|(_, o)| std::cmp::Reverse(o.max_hp));

    println!(
        "{:<8} {:<30} {:<7} {:<6} {:>7} {:<5} {:<6} {:<6} {}",
        "id", "name", "maxhp", "min", "%drop", "boss?", "minion", "strong", "labels"
    );
    for (id, o) in list {
        let t = o.object_type as i32;
        let name = assets
            .object_name(t)
            .unwrap_or_else(|| format!("type#{}", o.object_type));
        let labels = assets.object_labels(t).unwrap_or_default();
        let pct = if o.max_hp > 0 {
            100.0 * (o.max_hp - o.min_hp) as f64 / o.max_hp as f64
        } else {
            0.0
        };
        let is_minion = assets.is_minion(t);
        let is_strong = assets.is_strong_boss_like(t);
        let is_bosslike = assets.is_boss_like(t);
        // Mirror TrackedObject::is_boss classification.
        let class = assets.object_class(t).unwrap_or_default();
        let known_non_char = !class.is_empty() && class != "Character";
        let verdict = if assets.is_curated_boss(t) {
            true
        } else if is_minion {
            is_strong
        } else if is_bosslike {
            true
        } else if !labels.trim().is_empty() {
            false
        } else if known_non_char {
            false
        } else {
            o.max_hp >= 10_000
        };
        println!(
            "type={:<6} {:<8} {:<30} {:<7} {:<6} {:>6.1}% {:<5} {:<6} {:<6} {}",
            t,
            id,
            name.chars().take(30).collect::<String>(),
            o.max_hp,
            o.min_hp,
            pct,
            verdict,
            is_minion,
            is_strong,
            labels
        );
    }
}

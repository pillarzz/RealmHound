//! Diagnose "Unknown (out of range)" combat participants from a `.rhcap`.
//!
//! Replays a capture and, for every enemy/boss that took `DamagePacket` damage,
//! reports the raw attacker object ids that fail to resolve to a *named player*
//! (mirroring the tracker's Unresolved -> "Unknown (out of range)" collapse),
//! plus enough context (Update metadata, summon-owner links) to explain WHY.
//!
//! Usage:
//!   cargo run -p realmhound-core --example diagnose_unknown -- <path-to.rhcap>

use std::collections::HashMap;
use std::path::PathBuf;

use realmhound_core::assets::get_asset_manager;
use realmhound_core::capture::read_capture;
use realmhound_core::protocol::data::{ObjectStatusData, StatType};
use realmhound_core::protocol::{parse_packet, ParsedPacket};
use realmhound_core::stream::{parse_tcp_segment, TcpReassembler};

#[derive(Default, Clone)]
struct ObjInfo {
    object_type: u16,
    name: Option<String>,
    max_hp: i64,
    seen_in_update: bool,
    /// Ever appeared as a ServerPlayerShoot `owner_id` (i.e. acted as a summon).
    was_sps_owner: bool,
    /// Ever appeared as a ServerPlayerShoot `summoner_id` (i.e. owns a summon).
    was_sps_summoner: bool,
}

fn name_from_status(status: &ObjectStatusData) -> Option<String> {
    for stat in &status.stats {
        if stat.stat_type_id == StatType::Name as u8 {
            if let Some(n) = &stat.string_stat_value {
                if !n.is_empty() {
                    return Some(n.clone());
                }
            }
        }
    }
    None
}

fn main() {
    let Some(arg) = std::env::args().nth(1) else {
        eprintln!("usage: diagnose_unknown <path-to.rhcap>");
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

    let assets = get_asset_manager();
    let assets_dir = realmhound_core::assets::default_assets_dir();
    if assets_dir.exists() {
        assets.set_assets_dir(&assets_dir);
    } else {
        eprintln!(
            "(assets dir {} not found - boss names won't resolve)",
            assets_dir.display()
        );
    }

    let mut objects: HashMap<i32, ObjInfo> = HashMap::new();
    // summon entity id (SPS owner) -> summoner (player) id.
    let mut summon_owner: HashMap<i32, i32> = HashMap::new();
    // ServerPlayerShoot bullet_id -> summoner (player) id.
    let mut sps_bullets: HashMap<i16, i32> = HashMap::new();
    // target id -> attacker id -> (damage, hits)
    let mut dmg: HashMap<i32, HashMap<i32, (i64, u64)>> = HashMap::new();

    let mut reassembler = TcpReassembler::new();
    let mut damage_packets = 0u64;
    let mut sps_packets = 0u64;
    // object_id==0 DamagePacket analysis
    let mut zero_owner_dmg = 0u64;
    let mut zero_owner_bullet0 = 0u64;
    let mut zero_owner_bulletN = 0u64;
    let mut zero_owner_bullet_matches_sps = 0u64;

    let note_status =
        |objects: &mut HashMap<i32, ObjInfo>, status: &ObjectStatusData, otype: Option<u16>| {
            let e = objects.entry(status.object_id).or_default();
            if let Some(t) = otype {
                e.object_type = t;
                e.seen_in_update = true;
            }
            for stat in &status.stats {
                if stat.stat_type_id == StatType::MaxHP as u8 {
                    e.max_hp = stat.stat_value as i64;
                }
            }
            if let Some(n) = name_from_status(status) {
                e.name = Some(n);
            }
        };

    for rp in &raw {
        let Some(segment) = parse_tcp_segment(rp) else {
            continue;
        };
        for packet in reassembler.process_segment(&segment) {
            match parse_packet(packet.packet_type(), &packet.payload) {
                Some(ParsedPacket::Update(u)) => {
                    for obj in &u.new_objects {
                        note_status(&mut objects, &obj.status, Some(obj.object_type));
                    }
                }
                Some(ParsedPacket::NewTick(t)) => {
                    for status in &t.statuses {
                        note_status(&mut objects, status, None);
                    }
                }
                Some(ParsedPacket::Damage(d)) => {
                    damage_packets += 1;
                    if d.object_id == 0 {
                        zero_owner_dmg += 1;
                        if d.bullet_id == 0 {
                            zero_owner_bullet0 += 1;
                        } else {
                            zero_owner_bulletN += 1;
                            if sps_bullets.contains_key(&(d.bullet_id as i16)) {
                                zero_owner_bullet_matches_sps += 1;
                            }
                        }
                    }
                    let entry = dmg
                        .entry(d.target_id)
                        .or_default()
                        .entry(d.object_id)
                        .or_default();
                    entry.0 += d.damage_amount as i64;
                    entry.1 += 1;
                }
                Some(ParsedPacket::ServerPlayerShoot(s)) => {
                    sps_packets += 1;
                    // owner = summon entity, summoner = player (per verified captures).
                    // Match the tracker: only learn ownership for genuine summons.
                    if s.summoner_id != 0 && s.summoner_id != s.owner_id {
                        summon_owner.insert(s.owner_id, s.summoner_id);
                    }
                    sps_bullets.insert(s.bullet_id, s.summoner_id);
                    objects.entry(s.owner_id).or_default().was_sps_owner = true;
                    if s.summoner_id != 0 {
                        objects.entry(s.summoner_id).or_default().was_sps_summoner = true;
                    }
                }
                _ => {}
            }
        }
    }

    println!(
        "Objects seen: {}   Damage packets: {}   ServerPlayerShoot: {}\n",
        objects.len(),
        damage_packets,
        sps_packets
    );
    println!(
        "object_id==0 DamagePackets: {} ({:.1}% of all)  |  bullet_id==0: {}  bullet_id!=0: {}  (of those, matched a ServerPlayerShoot bullet: {})\n",
        zero_owner_dmg,
        100.0 * zero_owner_dmg as f64 / damage_packets.max(1) as f64,
        zero_owner_bullet0,
        zero_owner_bulletN,
        zero_owner_bullet_matches_sps,
    );

    let resolve = |objects: &HashMap<i32, ObjInfo>, id: i32| -> i32 {
        summon_owner.get(&id).copied().unwrap_or(id)
    };
    let is_named = |objects: &HashMap<i32, ObjInfo>, id: i32| -> bool {
        objects
            .get(&id)
            .and_then(|o| o.name.as_ref())
            .map(|n| !n.is_empty())
            .unwrap_or(false)
    };

    // Build a per-target summary, keeping only targets that look like a boss
    // (asset label) OR that have any unresolved-attacker damage worth explaining.
    struct TargetRow {
        target_id: i32,
        boss_name: String,
        is_boss: bool,
        total_dmg: i64,
        no_owner: (i64, u64),               // attacker id 0
        sentinel: (i64, u64),               // attacker id 0xFFFFFF
        real_unknown: Vec<(i32, i64, u64)>, // real unnamed attacker id, dmg, hits
    }
    let mut rows: Vec<TargetRow> = Vec::new();

    for (&target_id, attackers) in &dmg {
        let otype = objects
            .get(&target_id)
            .map(|o| o.object_type as i32)
            .unwrap_or(0);
        let boss_name = assets
            .object_name(otype)
            .unwrap_or_else(|| format!("type#{otype}"));
        let is_boss = otype != 0
            && (assets.is_boss_like(otype)
                || objects
                    .get(&target_id)
                    .map(|o| o.max_hp >= 10_000)
                    .unwrap_or(false));

        let mut total = 0i64;
        let mut no_owner = (0i64, 0u64);
        let mut sentinel = (0i64, 0u64);
        let mut real: HashMap<i32, (i64, u64)> = HashMap::new();
        for (&attacker, &(d, h)) in attackers {
            total += d;
            let eff = resolve(&objects, attacker);
            if is_named(&objects, eff) {
                continue;
            }
            if eff == 0 {
                no_owner.0 += d;
                no_owner.1 += h;
            } else if eff == 16_777_215 {
                sentinel.0 += d;
                sentinel.1 += h;
            } else {
                let u = real.entry(eff).or_default();
                u.0 += d;
                u.1 += h;
            }
        }
        let real_total: i64 = real.values().map(|v| v.0).sum();
        if real_total == 0 && no_owner.0 == 0 && sentinel.0 == 0 {
            continue;
        }
        let mut real_unknown: Vec<_> = real.into_iter().map(|(k, v)| (k, v.0, v.1)).collect();
        real_unknown.sort_by(|a, b| b.1.cmp(&a.1));
        rows.push(TargetRow {
            target_id,
            boss_name,
            is_boss,
            total_dmg: total,
            no_owner,
            sentinel,
            real_unknown,
        });
    }

    // Sort so targets with REAL unnamed-player damage (the actual bug) come first.
    rows.sort_by(|a, b| {
        let ar: i64 = a.real_unknown.iter().map(|x| x.1).sum();
        let br: i64 = b.real_unknown.iter().map(|x| x.1).sum();
        br.cmp(&ar)
    });

    println!("=== BOSS targets with unresolved-attacker damage ===");
    println!("(no_owner = attacker id 0; sentinel = 0xFFFFFF; REAL_UNKNOWN = named-player resolution failed)\n");
    for row in rows.iter().filter(|r| r.is_boss).take(30) {
        let real_total: i64 = row.real_unknown.iter().map(|x| x.1).sum();
        println!(
            "TARGET {} '{}'{}  total={}  no_owner={}({}h)  sentinel={}({}h)  REAL_UNKNOWN={} in {} id(s)",
            row.target_id,
            row.boss_name,
            if row.is_boss { " [BOSS]" } else { "" },
            row.total_dmg,
            row.no_owner.0,
            row.no_owner.1,
            row.sentinel.0,
            row.sentinel.1,
            real_total,
            row.real_unknown.len(),
        );
        for (eff_id, d, h) in row.real_unknown.iter().take(12) {
            let info = objects.get(eff_id).cloned().unwrap_or_default();
            let remapped = summon_owner.contains_key(eff_id);
            let otype = info.object_type as i32;
            let tname = if otype != 0 {
                assets
                    .object_name(otype)
                    .unwrap_or_else(|| format!("type#{otype}"))
            } else {
                "no-object-type".to_string()
            };
            println!(
                "    REAL attacker id={:<8} dmg={:<6} hits={:<4} name={:<16} obj_type={} '{}' seen_in_update={} sps_owner={} sps_summoner={} remapped={}",
                eff_id, d, h,
                info.name.clone().unwrap_or_else(|| "<none>".into()),
                otype, tname, info.seen_in_update, info.was_sps_owner, info.was_sps_summoner, remapped,
            );
        }
    }
}

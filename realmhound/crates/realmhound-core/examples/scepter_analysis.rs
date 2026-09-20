//! Analyze scepter fights from a .rhcap capture.
//! Counts UseItem (ability activations), ServerPlayerShoot, Damage, and Aoe
//! packets per Sprite World run to determine whether scepter damage is
//! observable in packet data.
//!
//! Usage:
//!   cargo run -p realmhound-core --example scepter_analysis -- <path.rhcap>

use std::collections::HashMap;
use std::path::PathBuf;

use realmhound_core::capture::read_capture;
use realmhound_core::protocol::{parse_packet, ParsedPacket};
use realmhound_core::stream::{parse_tcp_segment, TcpReassembler};

fn main() {
    let Some(arg) = std::env::args().nth(1) else {
        eprintln!("usage: scepter_analysis <path.rhcap>");
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
    let mut current_map = String::new();
    let mut map_index = 0u32;

    // Per-map tallies
    let mut use_item_count = 0u32;
    let mut use_item_types: HashMap<i32, u32> = HashMap::new();
    let mut server_player_shoot = 0u32;
    let mut enemy_hit = 0u32;
    let mut damage_count = 0u32;
    let mut damage_from_local = 0u32;
    let mut aoe_count = 0u32;
    let mut local_object_id: Option<i32> = None;

    // Damage from server player shoot (ability projectiles)
    let mut sps_damage_total = 0i32;
    let mut sps_bullet_ids: Vec<(u8, i32)> = Vec::new(); // (bullet_id, damage)

    // Track damage packets with details
    let mut damage_details: Vec<String> = Vec::new();
    let mut aoe_details: Vec<String> = Vec::new();

    let flush_map = |map: &str,
                     idx: u32,
                     uses: u32,
                     use_types: &HashMap<i32, u32>,
                     sps: u32,
                     eh: u32,
                     dmg: u32,
                     dmg_local: u32,
                     aoes: u32,
                     sps_dmg: i32,
                     dmg_det: &[String],
                     aoe_det: &[String]| {
        if map.is_empty() {
            return;
        }
        println!("=== Map #{idx}: {map} ===");
        println!("  UseItem:            {uses}");
        for (item_type, count) in use_types {
            println!("    item 0x{item_type:x}: {count} uses");
        }
        println!("  ServerPlayerShoot:  {sps}");
        println!("  EnemyHit (local):   {eh}");
        println!("  Damage packets:     {dmg} (from local player: {dmg_local})");
        println!("  Aoe packets:        {aoes}");
        if sps_dmg > 0 {
            println!("  SPS total damage:   {sps_dmg}");
        }
        for d in dmg_det.iter().take(20) {
            println!("    {d}");
        }
        for a in aoe_det.iter().take(20) {
            println!("    {a}");
        }
        println!();
    };

    for rp in &raw {
        let Some(segment) = parse_tcp_segment(rp) else {
            continue;
        };
        for packet in reassembler.process_segment(&segment) {
            match parse_packet(packet.packet_type(), &packet.payload) {
                Some(ParsedPacket::MapInfo(mi)) => {
                    // Flush previous map
                    flush_map(
                        &current_map,
                        map_index,
                        use_item_count,
                        &use_item_types,
                        server_player_shoot,
                        enemy_hit,
                        damage_count,
                        damage_from_local,
                        aoe_count,
                        sps_damage_total,
                        &damage_details,
                        &aoe_details,
                    );

                    current_map = mi.name.clone();
                    map_index += 1;
                    use_item_count = 0;
                    use_item_types.clear();
                    server_player_shoot = 0;
                    enemy_hit = 0;
                    damage_count = 0;
                    damage_from_local = 0;
                    aoe_count = 0;
                    sps_damage_total = 0;
                    sps_bullet_ids.clear();
                    damage_details.clear();
                    aoe_details.clear();
                    local_object_id = None;
                }
                Some(ParsedPacket::CreateSuccess(cs)) => {
                    local_object_id = Some(cs.object_id);
                    println!(
                        "  [Map #{map_index}] Local player object_id = {}",
                        cs.object_id
                    );
                }
                Some(ParsedPacket::UseItem(ui)) => {
                    use_item_count += 1;
                    *use_item_types.entry(ui.slot_object.item_type).or_default() += 1;
                }
                Some(ParsedPacket::ServerPlayerShoot(sps)) => {
                    server_player_shoot += 1;
                    if Some(sps.owner_id) == local_object_id {
                        sps_damage_total += sps.damage as i32;
                        sps_bullet_ids.push((sps.bullet_id as u8, sps.damage as i32));
                    }
                }
                Some(ParsedPacket::EnemyHit(eh_pkt)) => {
                    enemy_hit += 1;
                }
                Some(ParsedPacket::Damage(d)) => {
                    damage_count += 1;
                    if Some(d.object_id) == local_object_id {
                        damage_from_local += 1;
                        damage_details.push(format!(
                            "Damage: attacker={} target={} amount={} bullet={}",
                            d.object_id, d.target_id, d.damage_amount, d.bullet_id
                        ));
                    }
                }
                Some(ParsedPacket::Aoe(a)) => {
                    aoe_count += 1;
                    aoe_details.push(format!(
                        "Aoe: pos=({:.1},{:.1}) r={:.1} dmg={} orig=0x{:x} ap={}",
                        a.pos.x, a.pos.y, a.radius, a.damage, a.orig_type, a.armor_piercing
                    ));
                }
                _ => {}
            }
        }
    }

    // Flush last map
    flush_map(
        &current_map,
        map_index,
        use_item_count,
        &use_item_types,
        server_player_shoot,
        enemy_hit,
        damage_count,
        damage_from_local,
        aoe_count,
        sps_damage_total,
        &damage_details,
        &aoe_details,
    );
}

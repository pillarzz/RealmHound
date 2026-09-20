//! Offline combat-engine validation for `.rhcap` captures.
//!
//! Feeds a recorded raw-packet stream through the exact TCP reassembler and
//! packet parser used live, then drives the `CombatTracker` the same way the
//! processor does (Update -> spawn/remove, NewTick -> status/tick, Damage ->
//! attribution, MapInfo -> map change). It prints each reconstructed boss fight
//! so the DamagePacket attribution can be checked against the boss HP lost.
//!
//! Usage:
//!   cargo run -p realmhound-core --example combat_replay -- <path-to.rhcap>

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use realmhound_core::capture::read_capture;
use realmhound_core::combat::{CombatTracker, CompletedFight};
use realmhound_core::protocol::{parse_packet, ParsedPacket};
use realmhound_core::stream::{parse_tcp_segment, TcpReassembler};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn main() {
    let Some(arg) = std::env::args().nth(1) else {
        eprintln!("usage: combat_replay <path-to.rhcap>");
        std::process::exit(2);
    };
    let path = PathBuf::from(arg);

    // Load the object catalog so is_boss_like tag detection and boss-name
    // resolution work the same as they do in the live app.
    let assets_dir = realmhound_core::assets::default_assets_dir();
    if assets_dir.exists() {
        realmhound_core::assets::get_asset_manager().set_assets_dir(&assets_dir);
    } else {
        eprintln!(
            "(assets dir {} not found - boss names/tags may not resolve)",
            assets_dir.display()
        );
    }

    let raw = match read_capture(&path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("failed to read {}: {}", path.display(), e);
            std::process::exit(1);
        }
    };
    println!("Read {} raw packets from {}", raw.len(), path.display());

    let mut reassembler = TcpReassembler::new();
    let mut tracker = CombatTracker::new();
    let mut finished: Vec<CompletedFight> = Vec::new();

    // Synthetic monotonic clock (ms). Real timestamps are per-packet but the
    // tracker only needs monotonic ordering + a coarse tick cadence.
    let mut clock: i64 = 0;

    for rp in &raw {
        let Some(segment) = parse_tcp_segment(rp) else {
            continue;
        };
        for packet in reassembler.process_segment(&segment) {
            let ptype = packet.packet_type();
            let payload = &packet.payload;
            let incoming = packet.incoming;

            match parse_packet(ptype, payload) {
                Some(ParsedPacket::MapInfo(m)) => {
                    let name = if m.display_name.is_empty() {
                        m.name.clone()
                    } else {
                        m.display_name.clone()
                    };
                    finished.extend(tracker.on_map_change(&name, m.fp, clock));
                }
                Some(ParsedPacket::CreateSuccess(c)) => {
                    tracker.on_player_loaded(c.object_id, c.char_id);
                }
                Some(ParsedPacket::Update(u)) => {
                    for obj in &u.new_objects {
                        tracker.on_object_spawn(
                            obj.object_id(),
                            obj.object_type as i32,
                            &obj.status,
                            clock,
                        );
                    }
                    for &object_id in &u.drops {
                        if let Some(cf) = tracker.on_object_removed(object_id, clock) {
                            finished.push(cf);
                        }
                    }
                }
                Some(ParsedPacket::NewTick(t)) => {
                    clock += 200; // ~5 ticks/sec
                    for status in &t.statuses {
                        finished.extend(tracker.on_object_status(status.object_id, status, clock));
                    }
                    finished.extend(tracker.on_tick(clock));
                }
                Some(ParsedPacket::Damage(d)) if incoming => {
                    tracker.on_damage(d.target_id, d.object_id, d.damage_amount as i64, clock);
                }
                // Incoming summon/ally shots carry server-precomputed damage.
                Some(ParsedPacket::ServerPlayerShoot(s)) if incoming => {
                    tracker.on_ally_shot(
                        s.owner_id,
                        s.summoner_id,
                        s.bullet_id,
                        s.bullet_count,
                        s.damage,
                        s.container_type,
                        s.bullet_type,
                    );
                }
                // Outgoing local-player shots: roll damage so it can be matched
                // to the confirming EnemyHit below. Must run for every shot to
                // keep the damage RNG in sync with the client.
                Some(ParsedPacket::PlayerShoot(s)) if !incoming => {
                    tracker.on_local_shot(
                        s.weapon_id as i32,
                        s.projectile_id as i32,
                        s.bullet_id,
                        s.time,
                        s.player_position.x,
                        s.player_position.y,
                        s.angle,
                        clock,
                    );
                }
                // Outgoing local-player hits: the client carries no damage value,
                // so the engine resolves it from the correlated shot (weapon or
                // the local player's summon, per shooter_id).
                Some(ParsedPacket::EnemyHit(h)) if !incoming => {
                    tracker.on_local_hit(h.target_id, h.bullet_id, h.shooter_id, h.main_id, clock);
                }
                // Outgoing local `UseItem`: self-computed orb ability damage,
                // centered on the throw/aim position.
                Some(ParsedPacket::UseItem(u)) if !incoming => {
                    tracker.on_use_item(
                        u.slot_object.item_type,
                        u.use_item_position.x,
                        u.use_item_position.y,
                        clock,
                    );
                }
                _ => {}
            }
        }
    }

    // Flush anything still in progress at end of capture.
    finished.extend(tracker.on_disconnect(now_ms()));

    println!("\n=== Reconstructed boss fights: {} ===", finished.len());
    for (i, f) in finished.iter().enumerate() {
        let hp_lost = (f.boss_start_hp - if f.killed { 0 } else { f.boss_start_hp }).max(0);
        let attributed = f.total_damage();
        let pct = if f.boss_start_hp > 0 {
            (attributed as f64 / f.boss_start_hp as f64) * 100.0
        } else {
            0.0
        };
        println!(
            "\n[{i}] {} - boss={} (0x{:04X}) start_hp={} killed={} participants={}",
            f.dungeon,
            f.boss_name,
            f.boss_object_type as u16,
            f.boss_start_hp,
            f.killed,
            f.participants.len(),
        );
        println!(
            "     attributed_damage={attributed}  (~{pct:.1}% of start HP)  hp_lost_if_killed={hp_lost}"
        );
        for (rank, p) in f.participants.iter().take(20).enumerate() {
            println!(
                "     #{:>2} {:<20} id={:<10} dmg={:<10} hits={:<6} {:?}{}",
                rank + 1,
                p.name,
                p.object_id,
                p.damage,
                p.hits,
                p.provenance,
                if p.is_local { " (local)" } else { "" },
            );
        }
    }
}

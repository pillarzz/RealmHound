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
    let manager = realmhound_core::assets::get_asset_manager();

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
    // Summon-source creature spawns seen, by object type: (count, first, last).
    let mut summon_spawns: std::collections::HashMap<i32, (u32, i64, i64)> =
        std::collections::HashMap::new();

    // Optional: the shot multipliers the client applies to its own weapon rolls
    // (exaltation mastery x the seasonal challenge multipliers). In the live app
    // they come from the character cache; an offline replay has no cache, so pass
    // them here -- `combat_replay <capture> [mastery] [challenge]`.
    if let (Some(mastery), Some(challenge)) = (
        std::env::args().nth(2).and_then(|v| v.parse::<f32>().ok()),
        std::env::args().nth(3).and_then(|v| v.parse::<f32>().ok()),
    ) {
        println!("shot_damage_bonus: mastery={mastery} challenge={challenge}");
        tracker
            .set_shot_damage_bonus(realmhound_core::stats::ShotDamageBonus { mastery, challenge });
    }
    let mut finished: Vec<CompletedFight> = Vec::new();

    // What the client saw around the damage only packets can explain. Several
    // dungeons have mechanics that damage the boss *for* the player (Parasite
    // Chambers' barrels, the Galleon's cannonballs): the server credits the pusher,
    // so the hit arrives as a plain owner-credited `DamagePacket` with nothing else
    // pointing at it. Logging the objects that appeared or vanished as each such
    // packet landed is what names the source.
    let mut local_object_id: i32 = 0;
    let mut object_types: std::collections::HashMap<i32, i32> = std::collections::HashMap::new();
    let mut recent_objects: std::collections::VecDeque<(i64, i32, i32, bool)> =
        std::collections::VecDeque::new();
    let mut credited: Vec<(i64, u16, u16, i32)> = Vec::new();
    // Every object type seen, with a count and its first/last clock.
    let mut seen_types: std::collections::HashMap<i32, (u32, i64, i64)> =
        std::collections::HashMap::new();

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
                    local_object_id = c.object_id;
                    tracker.on_player_loaded(c.object_id, c.char_id);
                }
                Some(ParsedPacket::Update(u)) => {
                    for obj in &u.new_objects {
                        object_types.insert(obj.object_id(), obj.object_type as i32);
                        let entry = seen_types
                            .entry(obj.object_type as i32)
                            .or_insert((0, clock, clock));
                        entry.0 += 1;
                        entry.2 = clock;
                        recent_objects.push_back((clock, obj.object_type as i32, obj.object_id(), true));
                        // Creatures an item declares (a Kimono's spirits, a
                        // Summoner mace's bear) are spawned as ordinary objects;
                        // logging them shows whether RH ever saw the summon whose
                        // damage reaches it only as an owner-credited packet.
                        if !manager
                            .summon_sources(obj.object_type as i32)
                            .is_empty()
                        {
                            let entry = summon_spawns
                                .entry(obj.object_type as i32)
                                .or_insert((0u32, clock, clock));
                            entry.0 += 1;
                            entry.2 = clock;
                        }
                        tracker.on_object_spawn(
                            obj.object_id(),
                            obj.object_type as i32,
                            &obj.status,
                            clock,
                        );
                    }
                    for &object_id in &u.drops {
                        let otype = object_types.remove(&object_id).unwrap_or(0);
                        recent_objects.push_back((clock, otype, object_id, false));
                        if let Some(cf) = tracker.on_object_removed(object_id, clock) {
                            finished.push(cf);
                        }
                    }
                    while recent_objects
                        .front()
                        .is_some_and(|(t, ..)| clock - t > 2_000)
                    {
                        recent_objects.pop_front();
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
                    if d.object_id == local_object_id && local_object_id != 0 {
                        credited.push((clock, d.damage_amount, d.bullet_id, d.target_id));
                    }
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
                // Boss dialogue: the Marble Colossus survival-exit taunt and the
                // Moonlight Village dance-concluded line are both completion
                // signals the tracker consumes here.
                Some(ParsedPacket::Text(t)) => {
                    finished.extend(tracker.on_boss_text(
                        t.object_id,
                        Some(t.name.as_str()),
                        &t.text,
                        clock,
                    ));
                }
                // Moonlight Village's Leisurely Mode announcement.
                Some(ParsedPacket::Notification(n))
                    if realmhound_core::assets::is_mv_leisurely_mode_notification(&n.message) =>
                {
                    tracker.on_mv_leisurely_mode();
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

    // Objects whose names look like a mechanic that damages the boss *for* the
    // player (Parasite Chambers' barrels, the Galleon's cannons): if they are
    // ordinary world objects, they are spawned with the map and show up here.
    let mut mechanics: Vec<(i32, u32, i64, i64, String)> = seen_types
        .iter()
        .filter_map(|(&otype, &(count, first, last))| {
            let name = manager.object_name(otype)?;
            let lower = name.to_lowercase();
            ["barrel", "cannon", "bomb", "gunpowder"]
                .iter()
                .any(|key| lower.contains(key))
                .then_some((otype, count, first, last, name))
        })
        .collect();
    mechanics.sort_by(|a, b| b.1.cmp(&a.1));
    if !mechanics.is_empty() {
        println!(
            "\n=== objects that look like a boss-damaging mechanic ({} object types seen in total) ===",
            seen_types.len()
        );
        for (otype, count, first, last, name) in &mechanics {
            println!("     {name} (0x{otype:04X}) x{count}  first={first} last={last}");
        }
    }

    // Flush anything still in progress at end of capture.
    finished.extend(tracker.on_disconnect(now_ms()));

    // The damage no row explains -- and what the client saw as it landed (see the
    // note where `credited` is filled).
    if !credited.is_empty() {
        let total: i64 = credited.iter().map(|(_, amount, ..)| *amount as i64).sum();
        println!(
            "\n=== owner-credited damage packets: {} totalling {} ===",
            credited.len(),
            total
        );
        println!(
            "     (the server attributed these to the local player with no shot, \
             ability activation or creature of theirs behind them)"
        );
        let mut tally: std::collections::HashMap<i32, u32> = std::collections::HashMap::new();
        for (i, (clock, amount, bullet, target)) in credited.iter().enumerate() {
            let mut types: std::collections::BTreeSet<i32> = std::collections::BTreeSet::new();
            for (t, otype, _id, _spawned) in recent_objects.iter() {
                if (t - clock).abs() <= 600 && *otype != 0 {
                    types.insert(*otype);
                }
            }
            for otype in &types {
                *tally.entry(*otype).or_insert(0) += 1;
            }
            if i < 8 {
                let names: Vec<String> = types
                    .iter()
                    .map(|t| {
                        format!(
                            "{} (0x{:X})",
                            manager
                                .object_name(*t)
                                .unwrap_or_else(|| "unnamed".to_string()),
                            *t
                        )
                    })
                    .collect();
                println!(
                    "     #{:<3} t={:<8} dmg={:<6} bullet={:<3} target={:<6} objects nearby: {}",
                    i + 1,
                    clock,
                    amount,
                    bullet,
                    target,
                    if names.is_empty() {
                        "-".to_string()
                    } else {
                        names.join(", ")
                    }
                );
            }
        }
        let mut rows: Vec<(i32, u32)> = tally.into_iter().collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        println!("     objects seen within 600ms of a credited packet (by packet count):");
        for (otype, count) in rows.iter().take(10) {
            println!(
                "       in {}/{} packets: {} (0x{:04X})",
                count,
                credited.len(),
                manager
                    .object_name(*otype)
                    .unwrap_or_else(|| "unnamed".to_string()),
                *otype
            );
        }
    }

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
        if f.spirits > 0 {
            println!("     spirits_collected={}", f.spirits);
        }
        if f.leisurely {
            println!("     leisurely_mode=true");
        }
        if f.petless {
            println!("     challenge_mode=true (petless)");
        }
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
            if !p.summons.is_empty() {
                let rows: Vec<String> = p
                    .summons
                    .iter()
                    .map(|s| {
                        let who = if s.is_pet {
                            format!("pet 0x{:X}", s.pet_type)
                        } else {
                            format!("summon 0x{:X}", s.object_type)
                        };
                        let from = if s.source_item != 0 {
                            let name = manager
                                .object_name(s.source_item)
                                .unwrap_or_else(|| format!("0x{:X}", s.source_item));
                            format!(" from {name}")
                        } else {
                            String::new()
                        };
                        format!("{who}{from} dmg={} hits={}", s.damage, s.hits)
                    })
                    .collect();
                println!(
                    "          summon_damage={} summon_hits={} [{}]",
                    p.summon_damage,
                    p.summon_hits,
                    rows.join(", ")
                );
            }
            if p.packet_damage > 0 {
                println!(
                    "          packet_damage={} packet_hits={}",
                    p.packet_damage, p.packet_hits
                );
            }
        }
    }

    if !summon_spawns.is_empty() {
        println!("\n=== summon-source creatures spawned (type: count, first..last clock) ===");
        let mut rows: Vec<(i32, (u32, i64, i64))> = summon_spawns.into_iter().collect();
        rows.sort_by_key(|&(_, (count, _, _))| std::cmp::Reverse(count));
        for (obj_type, (count, first, last)) in rows {
            let name = manager
                .object_name(obj_type)
                .unwrap_or_else(|| format!("0x{obj_type:X}"));
            println!("   {name} (0x{obj_type:X}): {count} spawns, {first}..{last}ms");
        }
    }
}

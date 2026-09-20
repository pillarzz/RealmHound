//! Dump parsed ability effects for named items to sanity-check equip.xml
//! parsing and per-stat scaling accessors.
//!
//! Usage:
//!   cargo run -p realmhound-core --example ability_scan -- [assets_dir] [Item Name]...

use realmhound_core::assets::{find_assets_dir, get_asset_manager, AbilityEffect};

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    // Optional first arg: assets dir (else auto-detect).
    let dir = if !args.is_empty() && std::path::Path::new(&args[0]).join("xml").exists() {
        Some(std::path::PathBuf::from(args.remove(0)))
    } else {
        find_assets_dir()
    };
    let mgr = get_asset_manager();
    if let Some(d) = dir {
        println!("assets: {}", d.display());
        mgr.set_assets_dir(&d);
    }
    if !mgr.try_load() {
        eprintln!("failed to load assets: {:?}", mgr.last_error());
        return;
    }

    let names: Vec<String> = if args.is_empty() {
        vec![
            "Tome of Purification".into(),
            "Staff of Unholy Sacrifice".into(),
            "Orb of Conquest".into(),
            "Diplomatic Immunity".into(),
            "Doom Bow".into(),
            "Wand of Recompense".into(),
        ]
    } else {
        args
    };

    for name in &names {
        let id = if let Some(hex) = name.strip_prefix("0x").or_else(|| name.strip_prefix("0X")) {
            i32::from_str_radix(hex, 16).ok()
        } else {
            mgr.object_id_for_display_name(name)
        };
        let Some(id) = id else {
            println!("\n{name}: <unknown item>");
            continue;
        };
        let Some(eff) = mgr.ability_effects(id) else {
            println!("\n{name} (0x{id:x}): no ability effects parsed");
            continue;
        };
        println!("\n{name} (0x{id:x}):");
        if eff.applies_hex {
            println!("  applies Hex on enemy hits");
        }
        for g in &eff.poison_grenades {
            println!(
                "  Poison: impact@0={} total@0={} dur={}s scales={:?} min={}",
                g.impact_for(0),
                g.total_for(0),
                g.duration_s,
                g.scaling_stat,
                g.stat_mod_scaling_min
            );
        }
        if let Some(d) = eff.detonate_hex {
            println!(
                "  DetonateHex: base@0={} per@0={} scales={:?} min={}",
                d.base_for(0),
                d.per_stack_for(0),
                d.scaling_stat,
                d.stat_mod_scaling_min
            );
        }
        for e in &eff.effects {
            match e {
                AbilityEffect::ProjectileBurst {
                    min_hex,
                    max_hex,
                    burst,
                } => println!(
                    "  Burst[hex {min_hex}-{max_hex}]: {}-{} x{} scales={:?} min={} +{}/pt",
                    burst.min_damage as i32,
                    burst.max_damage as i32,
                    burst.num_shots,
                    burst.scaling_stat,
                    burst.stat_mod_scaling_min as i32,
                    burst.stat_mod_damage,
                ),
                AbilityEffect::Restore(r) => println!(
                    "  {}: {}@0 scales={:?} min={} +{}/pt",
                    if r.is_mp { "Magic" } else { "Heal" },
                    r.amount_for(0),
                    r.scaling_stat,
                    r.stat_mod_scaling_min as i32,
                    r.stat_mod_amount,
                ),
                AbilityEffect::Lightning(l) => {
                    let wis = 79;
                    println!(
                        "  Lightning@WIS{}: chain {}({}+{}) x{}({}+{}) targets  scales={:?} min={}",
                        wis,
                        l.chain_damage(wis),
                        l.chain_base_damage(),
                        l.chain_damage_bonus(wis),
                        l.chain_targets(wis),
                        l.chain_base_targets(),
                        l.chain_targets_bonus(wis),
                        l.scaling_stat,
                        l.stat_mod_scaling_min as i32,
                    );
                    if l.has_shockblast() {
                        let sv = l.scaling_stat.map(|_| wis).unwrap_or(0);
                        println!(
                            "    Shockblast: {} dmg (x{} triggers) x{} targets  radius {:.2}",
                            l.shock_damage(sv),
                            l.shock_triggers(),
                            l.shock_targets(wis),
                            l.shock_radius(wis),
                        );
                    }
                }
                AbilityEffect::Trap(t) => println!(
                    "  Trap: dmg@0={} bomb@0={} scales={:?}",
                    t.trap_damage_for(0),
                    t.bomb_damage_for(0),
                    t.scaling_stat,
                ),
                AbilityEffect::VampireBlast(v) => println!(
                    "  VampireBlast: dmg@0={} heal={} ignoreDef={} wisBase={} wisMin={}",
                    v.damage_for(0),
                    v.heal as i32,
                    v.ignore_def as i32,
                    v.wis_damage_base,
                    v.wis_min as i32,
                ),
                AbilityEffect::DamageNova(n) => println!(
                    "  DamageNova: {}-{} x{} waves radius {:.2} scales={:?} min={}",
                    n.min_damage as i32,
                    n.max_damage as i32,
                    n.activation_count as i32,
                    n.radius,
                    n.scaling_stat,
                    n.stat_mod_scaling_min as i32,
                ),
                AbilityEffect::StatBoost(b) => {
                    println!(
                        "  Buff: {}{} {} for {}s",
                        if b.amount >= 0 { "+" } else { "" },
                        b.amount,
                        b.stat,
                        b.duration
                    )
                }
                AbilityEffect::Condition(c) => {
                    println!("  Condition: {} for {}s", c.effect, c.duration)
                }
                AbilityEffect::Decoy { duration, speed } => {
                    println!("  Decoy: {duration}s speed={speed}")
                }
                AbilityEffect::Teleport { max_distance } => {
                    println!("  Teleport: {max_distance} tiles")
                }
                AbilityEffect::EffectBlast(eb) => println!(
                    "  EffectBlast: {} for {}s (WIS: +1s per {} above {}) r{}",
                    eb.condition, eb.base_duration, eb.wis_per_duration, eb.wis_min, eb.radius
                ),
                AbilityEffect::SpawnCreep(sc) => {
                    for proj in &sc.projectiles {
                        println!(
                            "  SpawnCreep: {} {}{}-{}{} (scale: {:?} +{}/pt above {})",
                            sc.display_name,
                            if !proj.name.is_empty() {
                                format!("({}) ", proj.name)
                            } else {
                                String::new()
                            },
                            proj.min_damage,
                            proj.max_damage,
                            if proj.armor_piercing { " AP" } else { "" },
                            sc.scaling_stat,
                            sc.stat_mod_damage,
                            sc.stat_mod_scaling_min
                        );
                    }
                }
                AbilityEffect::Shuriken(sh) => println!(
                    "  Shuriken: {}-{}{} x{} (scale: {:?} +{}/pt above {})",
                    sh.min_damage,
                    sh.max_damage,
                    if sh.armor_piercing { " AP" } else { "" },
                    sh.num_projectiles,
                    sh.scaling_stat,
                    sh.stat_mod_damage,
                    sh.stat_mod_scaling_min
                ),
                AbilityEffect::DashTrail { damage } => println!("  DashTrail: {damage}"),
            }
        }
    }
}

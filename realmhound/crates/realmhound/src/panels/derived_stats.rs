//! Derived character-stats UI: builds a `Loadout` from a cached character and
//! renders the computed effective attributes, weapon DPS, out-of-combat regen,
//! pet contribution, and loot/xp/dust boosts. Used by the "Attributes" block on
//! the Character Stats page and the Current-Character widget-bar tooltip.

use eframe::egui::{self, Color32, RichText};
use realmhound_core::assets::{get_asset_manager, ScalingStat};
use realmhound_core::stats::derived::{
    self, AbilityReadout, AbilitySegment, Attribute, BaseStats, DerivedStats, Loadout,
    PetAbilityContribution, Provenance, WeaponDamage,
};
use realmhound_core::vault::{CachedCharacter, CharacterCache, CharacterItem};

use crate::panels::character_card::{
    format_fame, stat_potion_id, FAME_SPRITE_ID, SLOT_SIZE, SLOT_SPACING,
};
use crate::rendering::{EmbeddedIcon, SpriteRenderer};

const HEADER_COLOR: Color32 = Color32::from_rgb(150, 200, 255);
/// Font size for the base-fame and fame-on-death lines folded into the top of
/// the ATTRIBUTES block (slightly smaller than the old footer fame).
const FAME_FONT_SIZE: f32 = 14.0;
/// Orange used for fame numbers, matching the character cards.
const FAME_COLOR: Color32 = Color32::from_rgb(255, 200, 100);

/// Character identity + fame lines folded into the top (and bottom) of the
/// ATTRIBUTES block on the Character Stats page: the leading skin/grave sprite
/// in the gear row, the name/id/class/maxed line, base fame, an optional
/// killed-by line, and the fame-on-death total shown below Bonuses. `None` for
/// the widget-bar tooltip, which renders attributes only.
pub struct StatsIdentity {
    pub skin_id: i32,
    pub tex1: u32,
    pub tex2: u32,
    pub fallback_initial: char,
    pub is_dead: bool,
    pub grave: Option<crate::tab_icons::TabIconSprite>,
    pub title: String,
    pub subtitle: String,
    pub base_fame: i64,
    pub killed_by: Option<String>,
    pub killer_icon: Option<i32>,
    pub fame_on_death: Option<i64>,
}

/// Draw a fame line: `<label> <icon> <number>` at [`FAME_FONT_SIZE`].
fn fame_line(ui: &mut egui::Ui, sr: &mut SpriteRenderer, label: &str, value: i64) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        ui.label(
            RichText::new(label)
                .size(FAME_FONT_SIZE)
                .color(Color32::WHITE),
        );
        let (rect, _) = ui.allocate_exact_size(
            egui::Vec2::splat(FAME_FONT_SIZE + 3.0),
            egui::Sense::hover(),
        );
        if ui.is_rect_visible(rect) {
            sr.draw_sprite_in_rect(ui, FAME_SPRITE_ID, rect);
        }
        ui.label(
            RichText::new(format_fame(value))
                .size(FAME_FONT_SIZE)
                .strong()
                .color(FAME_COLOR),
        );
    });
}

/// A horizontal separator that spans only the content rendered so far (the
/// cumulative content width) rather than the full available width. `ui.separator()`
/// stretches to the parent's max width, which pins the card's measured width and
/// stops it from shrinking when content is hidden (e.g. the ATTRIBUTES
/// Gear/Exalts/Crucible columns). The card's widest element (the gear row) sits
/// at the top, so this still spans the full card width.
fn content_separator(ui: &mut egui::Ui) {
    let width = ui.min_rect().width().max(1.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 6.0), egui::Sense::hover());
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.painter()
        .hline(rect.left()..=rect.right(), rect.center().y, stroke);
}
const LABEL_COLOR: Color32 = Color32::from_rgb(180, 180, 180);
const VALUE_COLOR: Color32 = Color32::from_rgb(235, 235, 235);
const NOTE_COLOR: Color32 = Color32::from_rgb(220, 180, 120);
/// Light-grey used for parenthetical annotations (e.g. weapon "(+5% enchant)")
/// so they read as secondary next to the white value.
const ANNOT_COLOR: Color32 = Color32::from_rgb(150, 150, 150);
/// Stat value at its class cap (gold), matching the character card.
const MAXED_COLOR: Color32 = Color32::from_rgb(255, 215, 0);
/// Stat value not yet at cap (green), matching the character card.
const UNMAXED_COLOR: Color32 = Color32::from_rgb(100, 220, 100);

/// Dimmed gear/exalt/crucible column colors so the bright Total column stands
/// out. Applied to the Gear / Exalts / Crucible headers and stat brackets.
const GEAR_DIM: Color32 = Color32::from_rgb(165, 145, 65);
const EXALT_DIM: Color32 = Color32::from_rgb(70, 120, 185);
const CRUCIBLE_DIM: Color32 = Color32::from_rgb(150, 95, 170);

/// Highest enemy defense in the game (5000), so the Enemy-DEF input caps at a
/// 4-digit number.
const MAX_ENEMY_DEF: i32 = 5000;

/// Extra live context the readout needs beyond the derived stats: gear slots,
/// seasonal/crucible markers, live-vs-stale state, the active loot boost, and
/// the account's Weapon-Proficiency drop-rate bonus (for the combined loot
/// total). Built via [`build_context`] where the cache + character are in hand.
#[derive(Debug, Clone, Default)]
pub struct BonusContext {
    pub loot_boost_active: bool,
    pub crucible_active: bool,
    pub seasonal: bool,
    /// False when this is a last-seen (not currently live) character; the
    /// readout is then dimmed with a "log in to update" note.
    pub is_live: bool,
    /// Exaltation Weapon-Proficiency drop-rate bonus, in percent.
    pub weapon_dr_pct: f32,
    /// Exaltation Fast Learner XP bonus, in percent (0/5/10/15/20).
    pub fast_learner_xp_pct: f32,
    /// Exaltation Armor Proficiency in-combat exit reduction, in seconds.
    pub armor_ic_secs: f32,
    /// The four equipped gear slots (weapon, ability, armor, ring) for the
    /// gear-slot row at the top of the readout.
    pub equipment: Vec<CharacterItem>,
    /// Active account-wide dust potion (Acc Dust Chance Day), if any. Account
    /// scope, so it is the same for every character. `None` when inactive.
    pub account_dust_boost: Option<AccountDustBoost>,
}

/// A live view of the account-wide dust potion for the Dust Boost readout.
#[derive(Debug, Clone, Copy)]
pub struct AccountDustBoost {
    /// Bonus magnitude in percent (e.g. 60.0 for +60% dust dropped).
    pub bonus_pct: f32,
    /// Seconds remaining until the potion expires.
    pub remaining_secs: u32,
    /// True when `remaining_secs` is a live estimate (from an activation packet)
    /// rather than an authoritative API value; the readout prefixes it with `~`.
    pub estimated: bool,
}

/// Build the readout context for a character. `loot_boost_active` is the live
/// loot-boost flag; `is_live` marks the currently-played character.
pub fn build_context(
    char: &CachedCharacter,
    cache: &CharacterCache,
    loot_boost_active: bool,
    is_live: bool,
) -> BonusContext {
    let weapon_dr_pct = realmhound_core::stats::exalt_proficiency::weapon_dr_pct(
        &cache.exaltation_stats,
        char.class_id as i32,
    ) as f32;
    let fast_learner_xp_pct = cache
        .exaltation_stats
        .get(&(char.class_id as i32))
        .map(realmhound_core::stats::exalt_proficiency::fast_learner_xp_pct)
        .unwrap_or(0) as f32;
    let armor_ic_secs = realmhound_core::stats::exalt_proficiency::armor_ic_tenths(
        &cache.exaltation_stats,
        char.class_id as i32,
    ) as f32
        / 10.0;
    let account_dust_boost = active_account_dust_boost(cache);
    BonusContext {
        loot_boost_active,
        crucible_active: char.crucible_active,
        seasonal: char.seasonal,
        is_live,
        weapon_dr_pct,
        fast_learner_xp_pct,
        armor_ic_secs,
        equipment: char.equipment.clone(),
        account_dust_boost,
    }
}

/// Resolve the currently-active account-wide dust potion from the cache's
/// accelerators, if any. Uses the current wall-clock time to age each
/// accelerator's absolute expiry; returns `None` when none are active.
fn active_account_dust_boost(cache: &CharacterCache) -> Option<AccountDustBoost> {
    let now = chrono::Utc::now().timestamp();
    cache
        .account_accelerators
        .iter()
        .filter(|acc| {
            acc.is_active(now) && realmhound_core::api::accelerator_info(acc.object_type).is_some()
        })
        .max_by_key(|acc| acc.expires_at)
        .and_then(|acc| {
            let info = realmhound_core::api::accelerator_info(acc.object_type)?;
            Some(AccountDustBoost {
                bonus_pct: info.bonus_pct,
                remaining_secs: acc.remaining_secs(now),
                estimated: acc.estimated,
            })
        })
}

/// Number of pet ability slots unlocked at a given rarity (0=Common .. 4=Divine):
/// Common 1, Uncommon/Rare 2, Legendary/Divine 3.
fn pet_unlocked_count(rarity: i32) -> usize {
    match rarity {
        0 => 1,
        1 | 2 => 2,
        _ => 3,
    }
}

/// Build a `Loadout` for `char` from its equipped gear, enchants, base stats,
/// equipped pet's abilities, and class exaltation bonuses.
pub fn build_loadout(char: &CachedCharacter, cache: &CharacterCache) -> Loadout {
    let s = &char.stats;
    let base = BaseStats {
        max_hp: s.max_hp,
        max_mp: s.max_mp,
        attack: s.attack,
        defense: s.defense,
        speed: s.speed,
        dexterity: s.dexterity,
        vitality: s.vitality,
        wisdom: s.wisdom,
    };

    // Equipment slots: 0=weapon, 1=ability, 2=armor, 3=ring. Preserve order so
    // index 0 stays the weapon for DPS derivation.
    let mut equipment = Vec::with_capacity(char.equipment.len());
    let mut enchants = Vec::with_capacity(char.equipment.len());
    for item in &char.equipment {
        equipment.push(item.item_id);
        enchants.push(item.enchant_ids.clone());
    }

    let pet = char
        .pet_instance_id
        .and_then(|id| cache.get_pet(id, char.seasonal))
        .map(|p| {
            // Only the pet's unlocked ability slots (by rarity) are active in
            // game; locked slots (shown at power 1) must not contribute.
            let unlocked = pet_unlocked_count(p.rarity);
            p.abilities
                .iter()
                .take(unlocked)
                .map(|a| (a.ability_type, a.power))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    // Exalt stat bonuses for the character's class (+1/level primary, +5/level
    // HP/MP). Damage exalt (server stat 113) is a live-only stat, so offline the
    // weapon multiplier stays exalt-neutral. Mastery weapon-damage % is derived
    // offline from the class's minimum stat exalt level.
    let class_exalt = cache.exaltation_stats.get(&(char.class_id as i32));
    let exalt_bonus = class_exalt
        .map(|ex| {
            let (hp, mp, att, def, spd, dex, vit, wis) = ex.stat_bonuses();
            BaseStats {
                max_hp: hp,
                max_mp: mp,
                attack: att,
                defense: def,
                speed: spd,
                dexterity: dex,
                vitality: vit,
                wisdom: wis,
            }
        })
        .unwrap_or_default();
    let mastery_dmg_pct = class_exalt
        .map(|ex| realmhound_core::stats::exalt_proficiency::mastery_dmg_tenths(ex) as f32 / 10.0)
        .unwrap_or(0.0);

    // Crucible season buffs apply only to characters carrying the marker.
    let crucible = if char.crucible_active {
        cache.active_crucible.clone()
    } else {
        None
    };

    Loadout {
        base,
        equipment,
        enchants,
        pet,
        exalt_damage_bonus: None,
        exalt_bonus,
        mastery_dmg_pct,
        crucible,
    }
}

/// Compute derived stats for a cached character.
pub fn compute(char: &CachedCharacter, cache: &CharacterCache) -> DerivedStats {
    let loadout = build_loadout(char, cache);
    let mut d = derived::derive(get_asset_manager(), &loadout);
    // Resolve the equipped pet's sprite id + type/name for the readout.
    if let Some(pet) = char
        .pet_instance_id
        .and_then(|id| cache.resolve_pet_identity(id, char.seasonal))
    {
        let sprite_id = if pet.skin > 0 { pet.skin } else { pet.pet_type };
        d.pet_skin = Some(sprite_id);
        d.pet_type = Some(pet.pet_type);
        // The stored `name` is the stale original-hatch family (e.g. "Blue Ant");
        // a re-skinned pet's real display name is the skin/type sprite's name
        // (e.g. "Red Heart"), so resolve it the same way the sprite does.
        let name = get_asset_manager()
            .object_name(sprite_id)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| pet.name.clone());
        if !name.is_empty() {
            d.pet_name = Some(name);
        }
    }
    d
}

fn label_row(ui: &mut egui::Ui, label: &str, value: String) {
    ui.label(RichText::new(label).color(LABEL_COLOR));
    ui.label(RichText::new(value).color(VALUE_COLOR).strong());
    ui.end_row();
}

/// Potion color for a scales-off stat label (e.g. "DEX"), matching `stat_color`.
fn stat_color_for_label(label: &str) -> Color32 {
    match label {
        "WIS" => stat_color(ScalingStat::Wisdom),
        "ATT" => stat_color(ScalingStat::Attack),
        "DEF" => stat_color(ScalingStat::Defense),
        "DEX" => stat_color(ScalingStat::Dexterity),
        "VIT" => stat_color(ScalingStat::Vitality),
        "SPD" => stat_color(ScalingStat::Speed),
        "MP" => stat_color(ScalingStat::MaxMp),
        _ => VALUE_COLOR,
    }
}

/// "Scales off:" grid row with each stat name in its potion color.
fn scales_off_row(ui: &mut egui::Ui, scales_off: &[&str]) {
    ui.label(RichText::new("Scales off:").color(LABEL_COLOR));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (i, s) in scales_off.iter().enumerate() {
            if i > 0 {
                ui.label(RichText::new(", ").color(VALUE_COLOR));
            }
            ui.label(RichText::new(*s).color(stat_color_for_label(s)).strong());
        }
    });
    ui.end_row();
}

/// Potion color for an ability's scaling-stat bonus segment, matching the stat
/// potion palette used across the app.
fn stat_color(stat: ScalingStat) -> Color32 {
    match stat {
        ScalingStat::Wisdom => Color32::from_rgb(90, 150, 255), // blue
        ScalingStat::Attack => Color32::from_rgb(230, 90, 230), // magenta
        ScalingStat::Defense => Color32::from_rgb(190, 190, 190), // grey
        ScalingStat::Dexterity => Color32::from_rgb(255, 165, 60), // orange
        ScalingStat::Vitality => Color32::from_rgb(230, 80, 80), // red
        ScalingStat::Speed => Color32::from_rgb(120, 220, 120), // green
        ScalingStat::MaxMp => Color32::from_rgb(255, 215, 0),   // gold
    }
}

/// Render one "Max damage per use" summary row: the plain total, each scaling
/// stat's contribution in its potion color, then a grey scenario note. Returns
/// the row response so a breakdown tooltip can be attached.
fn ability_damage_row(
    ui: &mut egui::Ui,
    total: i32,
    scaled: &[(ScalingStat, i32)],
    note: &str,
) -> egui::Response {
    let inner = ui.horizontal(|ui| {
        ui.label(RichText::new("Avg dmg/use:").color(LABEL_COLOR));
        ui.add_space(8.0);
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.label(
            RichText::new(format!("~{}", total))
                .color(VALUE_COLOR)
                .strong(),
        );
        for (stat, share) in scaled {
            ui.label(
                RichText::new(format!(" (+{})", share))
                    .color(stat_color(*stat))
                    .strong(),
            );
        }
        if !note.is_empty() {
            ui.label(RichText::new(format!("  {}", note)).color(NOTE_COLOR));
        }
    });
    let id = ui.make_persistent_id(("ability_dmg_row", note));
    ui.interact(inner.response.rect, id, egui::Sense::hover())
}

/// Detailed damage breakdown (scales-off + each damage line) for the summary's
/// hover tooltip. When the ability detonates hex, the max-stack formula and the
/// hex-stacking note fold in here.
fn ability_breakdown_hover(ui: &mut egui::Ui, ab: &AbilityReadout) {
    egui::Grid::new(ui.next_auto_id())
        .num_columns(2)
        .spacing([12.0, 3.0])
        .show(ui, |ui| {
            if !ab.scales_off.is_empty() {
                scales_off_row(ui, &ab.scales_off);
            }
            for line in ab.lines.iter().filter(|l| l.is_damage) {
                label_row_segments(ui, &line.label, &line.segments);
            }
        });
    if let Some(hex) = &ab.hex_blast {
        ui.add_space(2.0);
        let mult = hex.stack_multiplier;
        ui.label(
            RichText::new(if mult.abs() > f32::EPSILON {
                "Hex blast = (base + per-stack x stacks) x (1 + mult x stacks)"
            } else {
                "Hex blast = base + per-stack x stacks"
            })
            .color(LABEL_COLOR)
            .size(11.0),
        );
        let plugged = if mult.abs() > f32::EPSILON {
            format!(
                "= ({} + {} x {}) x (1 + {} x {}) = {}",
                hex.base, hex.per_stack, hex.max_stacks, mult, hex.max_stacks, hex.max_damage
            )
        } else {
            format!(
                "= {} + {} x {} = {}",
                hex.base, hex.per_stack, hex.max_stacks, hex.max_damage
            )
        };
        ui.label(RichText::new(plugged).color(VALUE_COLOR).size(11.0));
        ui.label(
            RichText::new(format!(
                "at {} hex stacks (per-enemy cap), built by weapon hits",
                hex.max_stacks
            ))
            .italics()
            .color(NOTE_COLOR)
            .size(11.0),
        );
    } else if ab.lines.iter().any(|l| l.label == "Hex blast") {
        ui.label(
            RichText::new("Hex blast needs weapon hits to build hex stacks.")
                .italics()
                .color(NOTE_COLOR)
                .size(11.0),
        );
    }
}

/// DPS breakdown for the weapon's hover tooltip: the damage range (with grey
/// enchant/mastery annotations), fire rate, projectile count, and armor piercing.
fn weapon_dps_tooltip(ui: &mut egui::Ui, w: &WeaponDamage) {
    egui::Grid::new(ui.next_auto_id())
        .num_columns(2)
        .spacing([12.0, 3.0])
        .show(ui, |ui| {
            let suffix = |ui: &mut egui::Ui| {
                if w.damage_mult > 1.0001 {
                    ui.label(
                        RichText::new(format!(
                            "  (+{:.0}% enchant)",
                            (w.damage_mult - 1.0) * 100.0
                        ))
                        .color(ANNOT_COLOR),
                    );
                }
                if w.mastery_pct > 0.001 {
                    ui.label(
                        RichText::new(format!("  (+{:.0}% mastery)", w.mastery_pct))
                            .color(ANNOT_COLOR),
                    );
                }
            };
            if w.extra_types.is_empty() {
                ui.label(RichText::new("Damage").color(LABEL_COLOR));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(
                        RichText::new(format!("{} - {}", w.min_damage, w.max_damage))
                            .color(VALUE_COLOR)
                            .strong(),
                    );
                    suffix(ui);
                });
                ui.end_row();
            } else {
                // Multi-type weapons: show each bullet type.
                ui.label(RichText::new("Bullet 1").color(LABEL_COLOR));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(
                        RichText::new(format!("{} - {}", w.min_damage, w.max_damage))
                            .color(VALUE_COLOR)
                            .strong(),
                    );
                    suffix(ui);
                });
                ui.end_row();
                for (i, &(emin, emax, _ap)) in w.extra_types.iter().enumerate() {
                    ui.label(RichText::new(format!("Bullet {}", i + 2)).color(LABEL_COLOR));
                    ui.label(
                        RichText::new(format!("{} - {}", emin, emax))
                            .color(VALUE_COLOR)
                            .strong(),
                    );
                    ui.end_row();
                }
            }
            label_row(ui, "Shots / sec", format!("{:.2}", w.shots_per_sec));
            if w.total_projectiles > 1 {
                label_row(ui, "Projectiles", w.total_projectiles.to_string());
            }
            if w.armor_piercing {
                label_row(ui, "Armor Piercing", "Yes".to_string());
            }
            for p in &w.procs {
                ui.label(RichText::new(p.label).color(LABEL_COLOR));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(
                        RichText::new(format!("+{:.0} dps", p.dps))
                            .color(VALUE_COLOR)
                            .strong(),
                    );
                    if p.continuous {
                        ui.label(
                            RichText::new(format!(
                                "  ({:.1} stacks x {}/s)",
                                p.activations_per_sec, p.damage_per_proc
                            ))
                            .color(ANNOT_COLOR),
                        );
                    } else {
                        let cadence = if p.activations_per_sec > 0.0 {
                            format!("every {}s", fmt_secs(1.0 / p.activations_per_sec))
                        } else if p.cooldown > 0.0 {
                            format!("every {}s", fmt_secs(p.cooldown))
                        } else {
                            "every shot".to_string()
                        };
                        ui.label(
                            RichText::new(format!("  ({} dmg {}", p.damage_per_proc, cadence))
                                .color(ANNOT_COLOR),
                        );
                        if p.proc_rate < 0.999 {
                            ui.label(
                                RichText::new(format!(", {:.0}% proc", p.proc_rate * 100.0))
                                    .color(ANNOT_COLOR),
                            );
                        }
                        ui.label(RichText::new(")").color(ANNOT_COLOR));
                    }
                });
                ui.end_row();
            }
        });
}

/// Format a seconds value dropping a trailing `.0` (e.g. `3.0 -> "3"`,
/// `0.4 -> "0.4"`), for compact proc-cadence labels.
fn fmt_secs(secs: f32) -> String {
    if (secs.fract()).abs() < 1e-6 {
        format!("{}", secs.round() as i64)
    } else {
        // Trim to at most two decimals, then strip trailing zeros.
        let s = format!("{:.2}", secs);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Pet ability list (name + fed power / dps) for the pet hover tooltip.
fn pet_abilities_tooltip(ui: &mut egui::Ui, pet: &[PetAbilityContribution]) {
    use realmhound_core::api::pet_ability_name;
    egui::Grid::new(ui.next_auto_id())
        .num_columns(2)
        .spacing([12.0, 3.0])
        .show(ui, |ui| {
            for a in pet {
                let dmg_note = if a.dps > 0.05 {
                    format!("lvl {}  ({:.1} dps)", a.level, a.dps)
                } else {
                    format!("lvl {}", a.level)
                };
                label_row(ui, pet_ability_name(a.ability_type), dmg_note);
            }
        });
}

/// Component breakdown grid for a bonus hover tooltip (e.g. Loot / XP Boost),
/// one "source  +N.N%" row per component.
fn bonus_components_tooltip(ui: &mut egui::Ui, components: &[(&str, f32)]) {
    egui::Grid::new(ui.next_auto_id())
        .num_columns(2)
        .spacing([12.0, 3.0])
        .show(ui, |ui| {
            for (name, pct) in components {
                ui.label(RichText::new(*name).color(LABEL_COLOR));
                ui.label(
                    RichText::new(format!("+{:.1}%", pct))
                        .color(VALUE_COLOR)
                        .strong(),
                );
                ui.end_row();
            }
        });
}

/// Ability line: label plus value segments, coloring stat-scaling portions with
/// their potion color and leaving base text in the default value color.
fn label_row_segments(ui: &mut egui::Ui, label: &str, segments: &[AbilitySegment]) {
    ui.label(RichText::new(label).color(LABEL_COLOR));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for seg in segments {
            let color = seg.stat.map(stat_color).unwrap_or(VALUE_COLOR);
            ui.label(RichText::new(&seg.text).color(color).strong());
        }
    });
    ui.end_row();
}

/// Reserved pixel widths for the four attribute columns, so switching gear
/// (e.g. HP crossing 1000) never resizes the grid and shifts the labels.
struct ColW {
    total: f32,
    gear: f32,
    exalt: f32,
    crucible: f32,
}

/// Width of `text` rendered with `font`, used to reserve fixed column widths.
fn text_width(ui: &egui::Ui, text: &str, font: egui::FontId) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_string(), font, Color32::WHITE)
        .size()
        .x
}

/// Compute the reserved column widths from the widest of the header label and a
/// max-digit sample value: Total 4 digits (+ potion icon), Gear 3, Exalts 2,
/// Crucible 3. Signs/parens are included in the samples.
fn attr_col_widths(ui: &egui::Ui) -> ColW {
    let body = egui::TextStyle::Body.resolve(ui.style());
    let hdr = egui::FontId::proportional(10.0);
    let pad = 8.0;
    let icon_w = 16.0 + 3.0;
    ColW {
        total: (icon_w + text_width(ui, "0000", body.clone())).max(text_width(
            ui,
            "Total",
            hdr.clone(),
        )) + pad,
        gear: text_width(ui, "(+000)", body.clone()).max(text_width(ui, "Gear", hdr.clone())) + pad,
        exalt: text_width(ui, "(+00)", body.clone()).max(text_width(ui, "Exalts", hdr.clone()))
            + pad,
        crucible: text_width(ui, "(+000)", body).max(text_width(ui, "Crucible", hdr)) + pad,
    }
}

/// Render `content` inside a fixed-width cell so the grid column can't resize
/// as the value's digit count changes.
fn fixed_cell(ui: &mut egui::Ui, width: f32, content: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, ui.spacing().interact_size.y),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_width(width);
            content(ui);
        },
    );
}

/// Renders a stat as three grid cells so the brackets align into columns:
/// `[potion icon + total] [gear] [exalt]`. The total is gold when the stat is
/// at its class cap (maxed) and green otherwise, matching the character card;
/// the gear bonus is yellow and the exalt bonus blue. Empty brackets still emit
/// a cell so the columns stay aligned across rows.
fn attr_cell(
    ui: &mut egui::Ui,
    sr: &mut SpriteRenderer,
    potion_label: &str,
    a: &Attribute,
    is_maxed: bool,
    show_crucible: bool,
    breakdown: bool,
    w: &ColW,
) {
    // Cell 1: potion icon + total value (fixed 4-digit width).
    fixed_cell(ui, w.total, |ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(16.0), egui::Sense::hover());
        if let Some(id) = stat_potion_id(potion_label) {
            sr.draw_outlined_sprite_in_rect(ui, id, rect);
        }
        let value_color = if is_maxed { MAXED_COLOR } else { UNMAXED_COLOR };
        ui.label(
            RichText::new(a.total().to_string())
                .color(value_color)
                .strong(),
        );
    });
    // With the breakdown off, only the Total column is shown.
    if !breakdown {
        return;
    }
    // Cell 2: gear bracket (yellow), fixed 3-digit width.
    fixed_cell(ui, w.gear, |ui| {
        let g = a.gear_display();
        if g != 0 {
            let sign = if g > 0 { "+" } else { "" };
            ui.label(RichText::new(format!("({}{})", sign, g)).color(GEAR_DIM));
        }
    });
    // Cell 3: exalt bracket (blue), fixed 2-digit width.
    fixed_cell(ui, w.exalt, |ui| {
        if a.exalt != 0 {
            ui.label(RichText::new(format!("(+{})", a.exalt)).color(EXALT_DIM));
        }
    });
    // Cell 4: crucible bracket (magenta), fixed 3-digit width, only when the
    // grid reserves it.
    if show_crucible {
        fixed_cell(ui, w.crucible, |ui| {
            if a.crucible != 0 {
                let sign = if a.crucible > 0 { "+" } else { "" };
                ui.label(RichText::new(format!("({}{})", sign, a.crucible)).color(CRUCIBLE_DIM));
            }
        });
    }
}

/// Draws a 16px stat-potion / pet icon; the cell is reserved even when `id` is
/// `None` so surrounding text keeps a consistent baseline.
fn icon16(ui: &mut egui::Ui, sr: &mut SpriteRenderer, id: Option<i32>) {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(16.0), egui::Sense::hover());
    if let Some(id) = id {
        sr.draw_outlined_sprite_in_rect(ui, id, rect);
    }
}

/// Draw an item/object sprite scaled to `size` using the same crisp,
/// integer-scaled, native-frame outline path as equipped weapon/ability icons
/// (a thin 1px baked outline, no fractional upscaling/deformation).
fn object_icon(ui: &mut egui::Ui, sr: &mut SpriteRenderer, id: i32, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::Vec2::splat(size), egui::Sense::hover());
    if id > 0 {
        sr.draw_outlined_sprite_in_rect_native(ui, id, rect);
    }
    resp
}

/// Colored status pill matching the mission-tooltip badge style: filled rounded
/// rect, CAPS white text centered vertically + horizontally with a 1px black
/// outline (e.g. SEASONAL / CRUCIBLE).
fn status_pill(ui: &mut egui::Ui, text: &str, r: u8, g: u8, b: u8) {
    let font = egui::FontId::proportional(11.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE);
    let size = galley.size() + egui::vec2(10.0, 4.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 3.0, Color32::from_rgb(r, g, b));

    let center = rect.center();
    let outline = Color32::from_black_alpha(220);
    for dx in [-1.0f32, 0.0, 1.0] {
        for dy in [-1.0f32, 0.0, 1.0] {
            if dx == 0.0 && dy == 0.0 {
                continue;
            }
            ui.painter().text(
                center + egui::vec2(dx, dy),
                egui::Align2::CENTER_CENTER,
                text,
                font.clone(),
                outline,
            );
        }
    }
    ui.painter().text(
        center,
        egui::Align2::CENTER_CENTER,
        text,
        font,
        Color32::WHITE,
    );
}

/// Seasonal / Crucible marker pills. Drawn just below the ATTRIBUTES header.
/// When `subtitle` is set (the `#ID • Class • ?/8` line), it is rendered inline
/// on the same row as the pills to keep the card narrow.
fn render_status_pills(ui: &mut egui::Ui, ctx: &BonusContext, subtitle: Option<&str>) {
    if !ctx.seasonal && !ctx.crucible_active && subtitle.is_none() {
        return;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if ctx.seasonal {
            status_pill(ui, "SEASONAL", 0x15, 0xdc, 0xa6);
        }
        if ctx.crucible_active {
            status_pill(ui, "CRUCIBLE", 0xdc, 0x21, 0x15);
        }
        if let Some(sub) = subtitle {
            ui.label(RichText::new(sub).color(Color32::GRAY));
        }
    });
    ui.add_space(3.0);
}

/// The last-seen note for stale (not currently live) readouts.
fn render_stale_note(ui: &mut egui::Ui, ctx: &BonusContext) {
    if !ctx.is_live {
        ui.label(
            RichText::new("This is your last seen live character. Log in to update stats")
                .italics()
                .color(Color32::from_rgb(235, 205, 90))
                .size(11.0),
        );
        ui.add_space(2.0);
    }
}

/// Equipped gear-slot row (weapon, ability, armor, ring) at the top of the
/// readout, reusing the character-card slot renderer. When `lead` is set, the
/// character's skin (and grave, if dead) is drawn before the equipment slots.
fn render_gear_row(
    ui: &mut egui::Ui,
    sr: &mut SpriteRenderer,
    ctx: &BonusContext,
    lead: Option<&StatsIdentity>,
) {
    if ctx.equipment.is_empty() && lead.is_none() {
        return;
    }
    let gear = crate::panels::character_card::CharacterCardWidget::compact();
    let empty = CharacterItem::empty();
    ui.horizontal(|ui| {
        if let Some(id) = lead {
            // Character skin, sized to match the equipment slots.
            let (rect, _) =
                ui.allocate_exact_size(egui::Vec2::splat(SLOT_SIZE), egui::Sense::hover());
            if ui.is_rect_visible(rect) {
                let skin_id = if id.skin_id > 0 {
                    id.skin_id
                } else {
                    id.fallback_initial as i32
                };
                if !sr.draw_dyed_outlined_character_sprite(ui, skin_id, rect, 6, id.tex1, id.tex2) {
                    ui.painter().rect_filled(
                        rect,
                        4.0,
                        crate::ui_colors::sprite_fallback_fill(ui.visuals()),
                    );
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        id.fallback_initial,
                        egui::FontId::proportional(18.0),
                        Color32::WHITE,
                    );
                }
            }
            ui.add_space(SLOT_SPACING);
            // Grave marker for dead characters.
            if let Some(grave) = id.grave {
                let (grect, _) =
                    ui.allocate_exact_size(egui::Vec2::splat(SLOT_SIZE), egui::Sense::hover());
                if ui.is_rect_visible(grect) {
                    sr.render_icon(ui, Some(grave), grect);
                }
                ui.add_space(SLOT_SPACING);
            }
        }
        for i in 0..4 {
            let item = ctx.equipment.get(i).unwrap_or(&empty);
            gear.render_item_slot(ui, item, sr);
            ui.add_space(crate::panels::character_card::SLOT_SPACING);
        }
    });
    ui.add_space(4.0);
}

/// A column-header cell for the attribute grid.
fn attr_header_cell(ui: &mut egui::Ui, width: f32, text: &str, color: Color32) {
    fixed_cell(ui, width, |ui| {
        ui.label(RichText::new(text).size(10.0).color(color));
    });
}

/// Whether the Character-Stats category identified by `key` is collapsed.
/// Collapsed state is persisted in egui memory and shared across frames.
pub fn category_collapsed(ctx: &egui::Context, key: &str) -> bool {
    let id = egui::Id::new(("stats_category_collapsed", key));
    ctx.data_mut(|d| d.get_persisted::<bool>(id).unwrap_or(false))
}

/// Render a clickable, collapsible category header used across the Character
/// Stats page. Returns `true` when the section is expanded (its body should be
/// drawn). Clicking toggles the collapsed state persisted under `key`.
pub fn collapsible_header(ui: &mut egui::Ui, key: &str, text: &str, size: f32) -> bool {
    let id = egui::Id::new(("stats_category_collapsed", key));
    let collapsed = category_collapsed(ui.ctx(), key);
    let arrow = if collapsed { "▶" } else { "▼" };
    let resp = ui
        .add(
            egui::Label::new(
                RichText::new(format!("{arrow} {text}"))
                    .strong()
                    .size(size)
                    .color(HEADER_COLOR),
            )
            .sense(egui::Sense::click()),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.clicked() {
        ui.ctx().data_mut(|d| d.insert_persisted(id, !collapsed));
    }
    !collapsed
}

/// Render the full Attributes readout. `target_def` is the mutable weapon-DPS
/// target defense (a selector is drawn next to the weapon). `ctx` supplies live
/// loot-boost / crucible state for the Bonuses section.
pub fn render_readout(
    ui: &mut egui::Ui,
    sr: &mut SpriteRenderer,
    d: &DerivedStats,
    target_def: &mut i32,
    target_def_text: &mut String,
    maxed: &[bool; 8],
    ctx: &BonusContext,
    hover_breakdown: bool,
    collapsible: bool,
    show_breakdown: bool,
    identity: Option<&StatsIdentity>,
) {
    if d.provenance == Provenance::Unavailable {
        ui.label(
            RichText::new("Asset data not loaded - attributes unavailable.").color(NOTE_COLOR),
        );
        return;
    }

    // Dim the whole readout when this is a last-seen (not currently live)
    // character; the stale note explains the "log in to update" state.
    if !ctx.is_live {
        ui.multiply_opacity(0.5);
    }
    render_stale_note(ui, ctx);

    if collapsible {
        if !collapsible_header(ui, "attributes", "ATTRIBUTES", 15.0) {
            return;
        }
    } else {
        ui.label(
            RichText::new("ATTRIBUTES")
                .strong()
                .size(15.0)
                .color(HEADER_COLOR),
        );
    }
    ui.add_space(3.0);
    render_status_pills(ui, ctx, identity.map(|id| id.subtitle.as_str()));

    // Character identity folded into the ATTRIBUTES block: name line (the
    // #id/class/maxed subtitle now rides on the status-pills row above), then
    // base fame, then (dead) a killed-by line -- all sitting between the status
    // pills and the gear row.
    if let Some(id) = identity {
        ui.label(
            RichText::new(&id.title)
                .strong()
                .size(16.0)
                .color(Color32::WHITE),
        );
        fame_line(ui, sr, "Base fame:", id.base_fame);
        if let Some(killed_by) = &id.killed_by {
            ui.horizontal(|ui| {
                if let Some(oid) = id.killer_icon {
                    let (r, _) =
                        ui.allocate_exact_size(egui::Vec2::splat(18.0), egui::Sense::hover());
                    if ui.is_rect_visible(r) {
                        sr.render_icon(ui, Some(crate::tab_icons::TabIconSprite::ObjectId(oid)), r);
                    }
                    ui.add_space(4.0);
                }
                ui.label(
                    RichText::new(format!("💀 Killed by: {}", killed_by)).color(Color32::WHITE),
                );
            });
        }
        ui.add_space(3.0);
    }

    render_gear_row(ui, sr, ctx, identity);
    if d.provenance == Provenance::Partial {
        ui.label(
            RichText::new("⚠ Some gear/enchants unknown - values may be understated.")
                .italics()
                .color(NOTE_COLOR),
        );
        ui.add_space(2.0);
    }

    // Two stats per row. With the breakdown on, each stat is 4 aligned cells
    // (total, gear, exalt, crucible) under repeated column headers. With it off,
    // only the bright Total column shows, and the card shrinks to that width.
    let a = &d.attributes;
    let w = attr_col_widths(ui);
    egui::Grid::new(ui.next_auto_id())
        .num_columns(if show_breakdown { 8 } else { 2 })
        .spacing([8.0, 3.0])
        .show(ui, |ui| {
            for _ in 0..2 {
                attr_header_cell(ui, w.total, "Total", MAXED_COLOR);
                if show_breakdown {
                    attr_header_cell(ui, w.gear, "Gear", GEAR_DIM);
                    attr_header_cell(ui, w.exalt, "Exalts", EXALT_DIM);
                    attr_header_cell(ui, w.crucible, "Crucible", CRUCIBLE_DIM);
                }
            }
            ui.end_row();
            attr_cell(ui, sr, "HP", &a.max_hp, maxed[0], true, show_breakdown, &w);
            attr_cell(ui, sr, "MP", &a.max_mp, maxed[1], true, show_breakdown, &w);
            ui.end_row();
            attr_cell(ui, sr, "ATT", &a.attack, maxed[2], true, show_breakdown, &w);
            attr_cell(
                ui,
                sr,
                "DEF",
                &a.defense,
                maxed[3],
                true,
                show_breakdown,
                &w,
            );
            ui.end_row();
            attr_cell(ui, sr, "SPD", &a.speed, maxed[4], true, show_breakdown, &w);
            attr_cell(
                ui,
                sr,
                "DEX",
                &a.dexterity,
                maxed[5],
                true,
                show_breakdown,
                &w,
            );
            ui.end_row();
            attr_cell(
                ui,
                sr,
                "VIT",
                &a.vitality,
                maxed[6],
                true,
                show_breakdown,
                &w,
            );
            attr_cell(ui, sr, "WIS", &a.wisdom, maxed[7], true, show_breakdown, &w);
            ui.end_row();
        });

    ui.add_space(6.0);
    content_separator(ui);

    // Regen (out of combat): total plus an icon breakdown of base (vit/wis),
    // regen enchant, and pet heal (pet shown only when it actually heals).
    ui.add_space(4.0);
    ui.label(
        RichText::new("Regeneration (out of combat)")
            .strong()
            .color(HEADER_COLOR),
    );
    egui::Grid::new(ui.next_auto_id())
        .num_columns(2)
        .spacing([12.0, 3.0])
        .show(ui, |ui| {
            regen_row_icons(
                ui,
                sr,
                "HP / sec",
                &d.hp_regen_breakdown,
                "VIT",
                d.pet_hp_per_sec(),
                d.pet_skin,
            );
            regen_row_icons(
                ui,
                sr,
                "MP / sec",
                &d.mp_regen_breakdown,
                "WIS",
                d.pet_mp_per_sec(),
                d.pet_skin,
            );
            // Combat Trigger: min post-DEF hit damage to enter in-combat (which
            // halves the vit/wis + pet portions above, but not gear regen).
            let trigger = derived::combat_trigger(a.defense.total());
            ui.label(RichText::new("Combat Trigger").color(LABEL_COLOR));
            ui.label(
                RichText::new(format!("{} damage", trigger))
                    .color(VALUE_COLOR)
                    .strong(),
            );
            ui.end_row();
            // Armor Proficiency shortens how long the class stays In Combat after
            // the last qualifying hit; it does not change the trigger threshold.
            if ctx.armor_ic_secs > 0.001 {
                ui.label(RichText::new("Armor Proficiency").color(LABEL_COLOR));
                ui.label(
                    RichText::new(format!("-{:.1}s in combat", ctx.armor_ic_secs))
                        .color(VALUE_COLOR)
                        .strong(),
                );
                ui.end_row();
            }
        });

    ui.add_space(6.0);
    content_separator(ui);
    ui.add_space(4.0);

    // Weapon: icon + name, damage/DPS, and the Target-DEF selector. Always shown
    // even when unimplemented or empty so the layout is stable.
    let weapon_item = ctx.equipment.first().filter(|i| !i.is_empty());
    let weapon_equipped = weapon_item.is_some();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let wid = d
            .weapon
            .as_ref()
            .map(|w| w.weapon_id)
            .or_else(|| weapon_item.map(|i| i.item_id));
        let name = d
            .weapon
            .as_ref()
            .map(|w| w.weapon_id)
            .or_else(|| weapon_item.map(|i| i.item_id))
            .and_then(|id| get_asset_manager().object_name(id));
        // Label first, then the item sprite, then the item name.
        ui.label(RichText::new("Weapon:").strong().color(HEADER_COLOR));
        if let Some(wid) = wid {
            let ench: Vec<i32> = weapon_item
                .map(|i| i.enchant_ids.iter().map(|&e| e as i32).collect())
                .unwrap_or_default();
            sr.render_item_sprite_with_enchants_native(ui, wid, &ench, 20.0);
        }
        match &name {
            Some(n) => {
                ui.label(RichText::new(n).strong().color(HEADER_COLOR));
            }
            None => {
                ui.label(
                    RichText::new("None")
                        .strong()
                        .color(Color32::from_rgb(220, 90, 90)),
                );
            }
        }
    });
    if let Some(w) = &d.weapon {
        // Only the two DPS figures are shown; damage range, fire rate and
        // projectile count fold into the DPS hover tooltip. A fixed label
        // column keeps both DPS numbers aligned under one another (and the
        // second on the same line as the Enemy DEF input).
        let body = egui::TextStyle::Body.resolve(ui.style());
        let label_w = text_width(ui, "DPS (5,000 DEF)", body) + 8.0;
        let dps0 = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                fixed_cell(ui, label_w, |ui| {
                    ui.label(RichText::new("DPS (0 DEF)").color(LABEL_COLOR));
                });
                ui.label(
                    RichText::new(format!("{:.0}", w.dps))
                        .color(VALUE_COLOR)
                        .strong(),
                )
            })
            .inner;
        if hover_breakdown {
            dps0.on_hover_ui(|ui| weapon_dps_tooltip(ui, w));
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            fixed_cell(ui, label_w, |ui| {
                ui.label(
                    RichText::new(format!("DPS ({} DEF)", fmt_thousands(*target_def)))
                        .color(LABEL_COLOR),
                );
            });
            let dpsn = ui.label(
                RichText::new(format!("{:.0}", w.dps_vs(*target_def)))
                    .color(VALUE_COLOR)
                    .strong(),
            );
            if hover_breakdown {
                dpsn.on_hover_ui(|ui| weapon_dps_tooltip(ui, w));
            }
            // Enemy DEF input sits to the right of the DPS-vs-target row.
            // Digits only, clamped to [0, 5000]; backed by a shared buffer so
            // the stats page and Current-Character widget stay in sync.
            ui.add_space(12.0);
            ui.label(RichText::new("Enemy DEF:").color(LABEL_COLOR));
            let resp = egui::Frame::NONE
                .outer_margin(egui::Margin {
                    left: 0,
                    right: 0,
                    top: -2,
                    bottom: 2,
                })
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(target_def_text)
                            .desired_width(48.0)
                            .char_limit(4),
                    )
                })
                .inner;
            if resp.changed() {
                let digits: String = target_def_text
                    .chars()
                    .filter(|c| c.is_ascii_digit())
                    .take(4)
                    .collect();
                if *target_def_text != digits {
                    *target_def_text = digits.clone();
                }
                *target_def = digits.parse::<i32>().unwrap_or(0).clamp(0, MAX_ENEMY_DEF);
            }
            if resp.lost_focus() {
                *target_def_text = target_def.to_string();
            }
        });
    } else if weapon_equipped {
        ui.label(
            RichText::new("Calculation not implemented yet")
                .italics()
                .color(NOTE_COLOR)
                .size(12.0),
        );
    }

    // Ability: theoretical self-computed damage for one ability use, scaled by
    // the current set (e.g. Command Cornea poison scales with MP). Always shown.
    ui.add_space(6.0);
    let ability_item = ctx.equipment.get(1).filter(|i| !i.is_empty());
    let ability_equipped = ability_item.is_some();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let aid = d
            .ability_readout
            .as_ref()
            .map(|ab| ab.item_id)
            .or_else(|| ability_item.map(|i| i.item_id));
        let name = aid.and_then(|id| get_asset_manager().object_name(id));
        // Label first, then the item sprite, then the item name.
        ui.label(RichText::new("Ability:").strong().color(HEADER_COLOR));
        if let Some(aid) = aid {
            let ench: Vec<i32> = ability_item
                .map(|i| i.enchant_ids.iter().map(|&e| e as i32).collect())
                .unwrap_or_default();
            sr.render_item_sprite_with_enchants_native(ui, aid, &ench, 20.0);
        }
        match &name {
            Some(n) => {
                ui.label(RichText::new(n).strong().color(HEADER_COLOR));
            }
            None => {
                ui.label(
                    RichText::new("None")
                        .strong()
                        .color(Color32::from_rgb(220, 90, 90)),
                );
            }
        }
    });
    if let Some(ab) = &d.ability_readout {
        let has_hex_blast = ab.lines.iter().any(|l| l.label == "Hex blast");
        let dmg_has_hover = ab.damage_per_use.is_some() && hover_breakdown;
        if let Some(dmg) = &ab.damage_per_use {
            // Only distinguish the two scenarios when they actually differ (AoE
            // abilities); single-target-only effects show one unqualified row.
            let split = dmg.single_target != dmg.total;
            let multi_note = if split {
                "(across several targets)"
            } else {
                ""
            };
            let r1 = ability_damage_row(ui, dmg.total, &dmg.scaled, multi_note);
            if hover_breakdown {
                r1.on_hover_ui(|ui| ability_breakdown_hover(ui, ab));
            }
            if split {
                let r2 =
                    ability_damage_row(ui, dmg.single_target, &dmg.single_scaled, "(for 1 target)");
                if hover_breakdown {
                    r2.on_hover_ui(|ui| ability_breakdown_hover(ui, ab));
                }
            }
        }
        // Hex-blast damage is folded into the max-potential estimate above (it
        // needs weapon-built stacks); the per-stack detail, formula, and note
        // live in the summary's hover tooltip.
        egui::Grid::new(ui.next_auto_id())
            .num_columns(2)
            .spacing([12.0, 3.0])
            .show(ui, |ui| {
                if !ab.scales_off.is_empty() {
                    scales_off_row(ui, &ab.scales_off);
                }
                for line in &ab.lines {
                    // Damage lines fold into the summary/hover when one exists.
                    if ab.damage_per_use.is_some() && line.is_damage {
                        continue;
                    }
                    label_row_segments(ui, &line.label, &line.segments);
                }
            });
        // The hex-stacking note lives in the summary hover; only show it inline
        // as a fallback when there is no damage tooltip to hold it.
        if has_hex_blast && !dmg_has_hover {
            ui.label(
                RichText::new("Hex blast needs weapon hits to build hex stacks.")
                    .italics()
                    .color(NOTE_COLOR)
                    .size(11.0),
            );
        }
        // Lethal Strike: show computed per-shot bonus using the target DEF input.
        if let Some(ls) = &ab.lethal_strike {
            let effective_def = (*target_def).max(0);
            let bonus = ls.bonus_vs(*target_def);
            let sc = stat_color(ls.scaling_stat);
            let flat_base = ls.flat - ls.flat_bonus;
            let perc_base = (ls.perc - ls.perc_bonus) * 100.0;
            let perc_bonus_disp = ls.perc_bonus * 100.0;
            let shots_per_sec = d.weapon.as_ref().map_or(0.0, |w| w.shots_per_sec);
            let weapon_projs = d.weapon.as_ref().map_or(1, |w| w.total_projectiles);

            // Additional shots: 2 server-side projectiles per weapon trigger,
            // firing at the same rate as the weapon (OnPlayerShootActivate).
            let extra_dps = bonus as f32 * ls.extra_shots as f32 * shots_per_sec;
            // Bonus weapon damage: +bonus per weapon projectile that hits.
            let weapon_bonus_dps = bonus as f32 * weapon_projs as f32 * shots_per_sec;
            // Total LS DPS contribution during the window.
            let total_ls_dps = extra_dps + weapon_bonus_dps;

            let resp = ui
                .horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let r = ui.label(RichText::new("Lethal Strike  ").color(LABEL_COLOR));
                    let r2 = if total_ls_dps > 0.05 {
                        let v = ui.label(
                            RichText::new(format!("+{:.0}", total_ls_dps))
                                .color(VALUE_COLOR)
                                .strong(),
                        );
                        let u = ui.label(RichText::new(" dps").color(LABEL_COLOR));
                        v | u
                    } else {
                        let v = ui.label(
                            RichText::new(format!("{}", bonus))
                                .color(VALUE_COLOR)
                                .strong(),
                        );
                        let u = ui.label(RichText::new(" dmg/shot").color(LABEL_COLOR));
                        v | u
                    };
                    ui.label(
                        RichText::new(format!("  for {:.1}s", ls.duration))
                            .color(NOTE_COLOR)
                            .size(12.0),
                    );
                    r | r2
                })
                .inner;
            let show_ls_formula = |ui: &mut egui::Ui| {
                // Bonus per shot formula.
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(RichText::new("Bonus/shot: ").color(LABEL_COLOR));
                    ui.label(RichText::new(format!("{}", flat_base)).color(VALUE_COLOR));
                    if ls.flat_bonus > 0 {
                        ui.label(RichText::new(format!("(+{})", ls.flat_bonus)).color(sc));
                    }
                    ui.label(RichText::new(" + ").color(VALUE_COLOR));
                    ui.label(RichText::new(format!("{:.0}%", perc_base)).color(VALUE_COLOR));
                    if perc_bonus_disp >= 0.5 {
                        ui.label(RichText::new(format!("(+{:.0}%)", perc_bonus_disp)).color(sc));
                    }
                    ui.label(
                        RichText::new(format!(" × {} DEF = ", effective_def)).color(VALUE_COLOR),
                    );
                    ui.label(
                        RichText::new(format!("{}", bonus))
                            .color(VALUE_COLOR)
                            .strong(),
                    );
                });
                ui.add_space(4.0);
                // Section 1: Additional shots.
                if ls.extra_shots > 0 {
                    ui.label(
                        RichText::new("Additional shots")
                            .strong()
                            .color(HEADER_COLOR)
                            .size(12.0),
                    );
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.label(
                            RichText::new(format!(
                                "{} bonus × {} shots × {:.2} shots/s = ",
                                bonus, ls.extra_shots, shots_per_sec
                            ))
                            .color(VALUE_COLOR),
                        );
                        ui.label(
                            RichText::new(format!("+{:.0} dps", extra_dps))
                                .color(VALUE_COLOR)
                                .strong(),
                        );
                    });
                }
                // Section 2: Bonus weapon damage.
                ui.label(
                    RichText::new("Bonus weapon damage")
                        .strong()
                        .color(HEADER_COLOR)
                        .size(12.0),
                );
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(
                        RichText::new(format!(
                            "{} bonus × {} proj × {:.2} shots/s = ",
                            bonus, weapon_projs, shots_per_sec
                        ))
                        .color(VALUE_COLOR),
                    );
                    ui.label(
                        RichText::new(format!("+{:.0} dps", weapon_bonus_dps))
                            .color(VALUE_COLOR)
                            .strong(),
                    );
                });
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(RichText::new("Total: ").color(LABEL_COLOR));
                    ui.label(
                        RichText::new(format!("+{:.0} dps", total_ls_dps))
                            .color(VALUE_COLOR)
                            .strong(),
                    );
                    ui.label(RichText::new(format!("  for {:.1}s", ls.duration)).color(NOTE_COLOR));
                });
            };
            if hover_breakdown {
                resp.on_hover_ui(|ui| {
                    ui.set_max_width(340.0);
                    ui.label(RichText::new("Lethal Strike").strong().color(HEADER_COLOR));
                    ui.add_space(4.0);
                    show_ls_formula(ui);
                });
            } else {
                show_ls_formula(ui);
            }
        }
    } else if ability_equipped {
        ui.label(
            RichText::new("Calculation not implemented yet")
                .italics()
                .color(NOTE_COLOR)
                .size(12.0),
        );
    }

    // Pet: name (+ Pet DPS inline). The ability list (name + fed power / dps)
    // folds into a hover tooltip on the pet icon/name. Only real attack
    // abilities produce DPS; buffs (Rising Fury, Savage, etc.) contribute none.
    if !d.pet.is_empty() || d.pet_name.is_some() {
        ui.add_space(6.0);
        let pet_dps = d.pet_dps();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(RichText::new("Pet:").strong().color(HEADER_COLOR));
            let icon_resp = d.pet_skin.map(|skin| object_icon(ui, sr, skin, 20.0));
            let pet_name = d.pet_name.clone().unwrap_or_else(|| "Unknown".to_string());
            let name_resp = ui.label(RichText::new(pet_name).color(VALUE_COLOR).strong());
            if pet_dps > 0.05 {
                ui.label(
                    RichText::new(format!("{:.1} dps", pet_dps))
                        .color(VALUE_COLOR)
                        .strong(),
                );
            }
            // Ability list on hover of the pet icon or name.
            if hover_breakdown && !d.pet.is_empty() {
                let mut hover = name_resp;
                if let Some(ic) = icon_resp {
                    hover = hover.union(ic);
                }
                hover.on_hover_ui(|ui| pet_abilities_tooltip(ui, &d.pet));
            }
        });
    }

    // Bonuses: combined loot/xp/dust totals plus BXP.
    render_bonuses(ui, sr, d, ctx);

    // Fame on death, folded below Bonuses (dead: actual dead fame; living:
    // theoretical fame the character would earn if it died now).
    if let Some(id) = identity {
        if let Some(fame_on_death) = id.fame_on_death {
            ui.add_space(4.0);
            let label = if id.is_dead {
                "Dead fame:"
            } else {
                "Fame on death:"
            };
            fame_line(ui, sr, label, fame_on_death);
        }
    }
}

/// Format an integer with thousands separators (e.g. 5000 -> "5,000").
fn fmt_thousands(n: i32) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        format!("-{}", out)
    } else {
        out
    }
}

/// Regen row with an icon breakdown: `total (potion base + enchant bonus + pet)`.
/// `base_potion` is the stat potion driving base regen (VIT for HP, WIS for MP).
/// The pet segment is shown only when the pet actually contributes (heal/magic
/// heal), and the enchant segment only when a regen enchant is present.
fn regen_row_icons(
    ui: &mut egui::Ui,
    sr: &mut SpriteRenderer,
    label: &str,
    r: &derived::RegenBreakdown,
    base_potion: &str,
    pet_per_sec: f32,
    pet_skin: Option<i32>,
) {
    ui.label(RichText::new(label).color(LABEL_COLOR));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        let total = r.total() + pet_per_sec;
        ui.label(
            RichText::new(format!("{:.1}", total))
                .color(VALUE_COLOR)
                .strong(),
        );
        ui.label(RichText::new("(").color(LABEL_COLOR));
        // Base (vit/wis potion).
        icon16(ui, sr, stat_potion_id(base_potion));
        ui.label(RichText::new(format!("{:.1}", r.base)).color(VALUE_COLOR));
        // Regen enchant.
        if r.enchant_bonus > 0.05 {
            ui.label(RichText::new("+").color(LABEL_COLOR));
            let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(16.0), egui::Sense::hover());
            if let Some(eid) = r.enchant_id {
                sr.draw_enchant_sprite_in_rect(ui, eid as u16, rect);
            }
            ui.label(RichText::new(format!("{:.1}", r.enchant_bonus)).color(VALUE_COLOR));
        }
        // Pet heal (only when the pet actually heals this resource).
        if pet_per_sec > 0.05 {
            ui.label(RichText::new("+").color(LABEL_COLOR));
            icon16(ui, sr, pet_skin);
            ui.label(RichText::new(format!("{:.1}", pet_per_sec)).color(VALUE_COLOR));
        }
        ui.label(RichText::new(")").color(LABEL_COLOR));
    });
    ui.end_row();
}

/// Bonuses: Loot Boost broken down by source (gear + exalts + crucible + active
/// clover, shown as separate components since the exact in-game stacking formula
/// is unknown), XP Boost, Dust Boost, and Battle-pass XP. Loot/XP/Dust rows are
/// always shown (0% when none).
fn render_bonuses(
    ui: &mut egui::Ui,
    sr: &mut SpriteRenderer,
    d: &DerivedStats,
    ctx: &BonusContext,
) {
    ui.add_space(6.0);
    content_separator(ui);
    ui.add_space(4.0);
    ui.label(RichText::new("Bonuses").strong().color(HEADER_COLOR));

    // Loot Boost components (percent), shown as separate sources rather than a
    // combined total: the exact way the game stacks gear, Weapon-Proficiency
    // exalt, crucible and an active Lucky Clover is not known, so we avoid
    // implying a single additive or multiplicative figure.
    let gear_loot = d.loot_boost_pct;
    let exalt_loot = ctx.weapon_dr_pct;
    let crucible_loot = d.crucible.as_ref().map(|c| c.loot_pct).unwrap_or(0.0);
    let clover_loot = if ctx.loot_boost_active { 50.0 } else { 0.0 };

    // Lucky Clover (0xcc2), XP Booster 20 min (0xc6b), Acc Dust Chance Day (0x2ef).
    // Loot Boost as a single approximate additive figure (the exact in-game
    // stacking formula is unknown, hence the "~"); components in the hover.
    let loot_total = gear_loot + exalt_loot + crucible_loot + clover_loot;
    let loot_components = [
        ("gear", gear_loot),
        ("exalts", exalt_loot),
        ("crucible", crucible_loot),
        ("clover", clover_loot),
    ];
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        object_icon(ui, sr, 0xcc2, 16.0);
        ui.label(RichText::new("Loot Boost:").color(LABEL_COLOR));
        ui.label(
            RichText::new(format!("~+{:.1}%", loot_total))
                .color(VALUE_COLOR)
                .strong(),
        )
        .on_hover_ui(|ui| bonus_components_tooltip(ui, &loot_components));
    });

    // Fold the base per-item XP bonus (equip.xml `<XPBonus>`) into the gear
    // line alongside gear-enchant XP so the total reflects both. Components fold
    // into the hover tooltip.
    let gear_xp = d.xp_boost_pct + d.xp_base_gear_pct;
    let crucible_xp = d.crucible.as_ref().map(|c| c.xp_pct).unwrap_or(0.0);
    let exalt_xp = ctx.fast_learner_xp_pct;
    let xp_components = [
        ("gear", gear_xp),
        ("exalts", exalt_xp),
        ("crucible", crucible_xp),
    ];
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        object_icon(ui, sr, 0xc6b, 16.0);
        ui.label(RichText::new("XP Boost:").color(LABEL_COLOR));
        ui.label(
            RichText::new(format!("+{:.1}%", gear_xp + crucible_xp + exalt_xp))
                .color(VALUE_COLOR)
                .strong(),
        )
        .on_hover_ui(|ui| bonus_components_tooltip(ui, &xp_components));
    });

    // Dust Boost: total (gear + account potion). The component breakdown folds
    // into a hover tooltip on the total (bullet text white; the potion's
    // remaining time greyed in parentheses).
    let dust_gear = d.dust_boost_pct;
    let dust_potion = ctx.account_dust_boost.map(|b| b.bonus_pct).unwrap_or(0.0);
    let account_boost = ctx.account_dust_boost;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        object_icon(ui, sr, 0x2ef, 16.0);
        ui.label(RichText::new("Dust Boost:").color(LABEL_COLOR));
        ui.label(
            RichText::new(format!("+{:.1}%", dust_gear + dust_potion))
                .color(VALUE_COLOR)
                .strong(),
        )
        .on_hover_ui(|ui| {
            ui.label(RichText::new(format!("•  gear: {:.1}%", dust_gear)).color(Color32::WHITE));
            match account_boost {
                Some(boost) => {
                    let time_str = format!(
                        "{}{} left",
                        if boost.estimated { "~" } else { "" },
                        crate::panels::character_card::format_loot_boost(boost.remaining_secs as u64)
                    );
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.label(
                            RichText::new(format!("•  account potion: {:.0}%", boost.bonus_pct))
                                .color(Color32::WHITE),
                        );
                        ui.label(RichText::new(format!("({})", time_str)).color(LABEL_COLOR));
                    });
                    ui.label(
                        RichText::new(if boost.estimated {
                            "Account-wide dust potion. Time left is an estimate; corrected on the next API refresh."
                        } else {
                            "Account-wide dust potion. Persists across characters; counts down in real time."
                        })
                        .color(NOTE_COLOR)
                        .size(11.0),
                    );
                }
                None => {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.label(RichText::new("•  account potion: ").color(Color32::WHITE));
                        ui.label(RichText::new("(none)").italics().color(NOTE_COLOR).size(11.0));
                    });
                }
            }
        });
    });
    if account_boost.is_some() && ctx.is_live {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(1));
    }

    // Battle-pass XP (crucible bonus), with the embedded battle-pass sprite.
    let bxp = d.crucible.as_ref().map(|c| c.bxp_pct).unwrap_or(0.0);
    if bxp != 0.0 {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(16.0), egui::Sense::hover());
            sr.draw_outlined_embedded_icon(ui, EmbeddedIcon::Bxp, rect);
            ui.label(RichText::new("BXP:").color(LABEL_COLOR));
            ui.label(
                RichText::new(format!("+{:.0}%", bxp))
                    .color(VALUE_COLOR)
                    .strong(),
            );
        });
    }
}

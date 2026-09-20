//! Pet Yard diagnostics panel.
//!
//! A read-only view over the pet identity/inventory cache. It exposes exactly
//! what RealmHound has stored for every known pet in both the regular and
//! seasonal pools, so pet-detection problems (wrong sprite, missing pet,
//! stale identity) can be diagnosed directly.
//!
//! Pets are account-wide: a pet's identity (skin/type/name) is shared across
//! all characters, but its inventory is tracked independently per pool. This
//! view surfaces cross-pool identity conflicts (the same instance id resolving
//! to a different sprite in each pool), which is the tell-tale sign of stale or
//! mis-detected identity data.

use eframe::egui::{self, Color32, RichText};
use realmhound_core::assets::get_asset_manager;
use realmhound_core::vault::{CachedCharacter, CachedPet};

use crate::panels::{AppAction, Panel, PanelContext};
use crate::rendering::SpriteRenderer;

const TILE_SIZE: f32 = 32.0;

#[derive(Default)]
pub struct PetYardPanel {
    filter: String,
}

impl PetYardPanel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sprite id the character card would render for a pet: the skin when set,
    /// otherwise the base pet type.
    fn resolved_sprite(pet: &CachedPet) -> i32 {
        if pet.skin > 0 {
            pet.skin
        } else {
            pet.pet_type
        }
    }

    fn matches_filter(&self, pet: &CachedPet) -> bool {
        let q = self.filter.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        let sprite = Self::resolved_sprite(pet);
        let sprite_name = get_asset_manager()
            .object_name(sprite)
            .unwrap_or_default()
            .to_lowercase();
        pet.instance_id.to_string().contains(&q)
            || pet.name.to_lowercase().contains(&q)
            || sprite.to_string().contains(&q)
            || sprite_name.contains(&q)
    }

    /// Characters (in the given pool) that currently have this pet equipped.
    fn equipped_by(chars: &[CachedCharacter], instance_id: i32, seasonal: bool) -> Vec<String> {
        chars
            .iter()
            .filter(|c| c.seasonal == seasonal && c.pet_instance_id == Some(instance_id))
            .map(|c| format!("{} #{}", c.class_name(), c.char_id))
            .collect()
    }

    fn render_pool(
        ui: &mut egui::Ui,
        title: &str,
        pets: &std::collections::HashMap<i32, CachedPet>,
        chars: &[CachedCharacter],
        seasonal: bool,
        panel: &PetYardPanel,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        ui.vertical(|ui| {
            ui.heading(title);
            ui.label(
                RichText::new(format!("{} pet(s) cached", pets.len()))
                    .weak()
                    .small(),
            );
            ui.add_space(4.0);

            let mut sorted: Vec<&CachedPet> = pets.values().collect();
            sorted.sort_by_key(|p| p.instance_id);

            let mut shown = 0;
            for pet in sorted {
                if !panel.matches_filter(pet) {
                    continue;
                }
                shown += 1;
                Self::render_pet(ui, pet, chars, seasonal, sprite_renderer);
                ui.add_space(6.0);
            }

            if shown == 0 {
                ui.add_space(8.0);
                ui.label(RichText::new("No matching pets.").weak());
            }
        });
    }

    fn render_pet(
        ui: &mut egui::Ui,
        pet: &CachedPet,
        chars: &[CachedCharacter],
        seasonal: bool,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        let fill = crate::ui_colors::card_fill(ui.visuals());
        let stroke = crate::ui_colors::card_stroke(ui.visuals());
        let sprite = Self::resolved_sprite(pet);
        let sprite_name = get_asset_manager()
            .object_name(sprite)
            .unwrap_or_else(|| "<unknown sprite>".to_string());

        egui::Frame::NONE
            .fill(fill)
            .stroke(egui::Stroke::new(1.0_f32, stroke))
            .corner_radius(6.0)
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // The exact sprite a character card resolves for this pet.
                    sprite_renderer.render_item_tile_sized(ui, sprite, None, &[], 40.0);
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        let display = if pet.name.is_empty() {
                            sprite_name.clone()
                        } else {
                            pet.name.clone()
                        };
                        ui.label(
                            RichText::new(format!("#{}  {}", pet.instance_id, display)).strong(),
                        );
                        ui.label(
                            RichText::new(format!(
                                "skin={}  type={}  → sprite {} ({})",
                                pet.skin, pet.pet_type, sprite, sprite_name
                            ))
                            .small(),
                        );
                        ui.label(
                            RichText::new(format!(
                                "{}  ·  {} slot(s)  ·  {}",
                                pet.rarity_name(),
                                pet.inventory_slots,
                                if pet.identity_from_api {
                                    "API-verified"
                                } else {
                                    "live-detected"
                                }
                            ))
                            .small()
                            .weak(),
                        );
                        let equipped = Self::equipped_by(chars, pet.instance_id, seasonal);
                        let equipped_text = if equipped.is_empty() {
                            "equipped by: none (cached from a past refresh)".to_string()
                        } else {
                            format!("equipped by: {}", equipped.join(", "))
                        };
                        ui.label(RichText::new(equipped_text).small().weak());
                        if !pet.abilities.is_empty() {
                            let abilities_text = pet
                                .abilities
                                .iter()
                                .map(|a| {
                                    format!(
                                        "#{} {} (pow {})",
                                        a.ability_type,
                                        a.ability_name(),
                                        a.power
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join("   ");
                            ui.label(
                                RichText::new(format!("abilities: {}", abilities_text))
                                    .small()
                                    .color(Color32::from_rgb(150, 190, 230)),
                            );
                        }
                    });
                });

                ui.add_space(6.0);
                Self::render_inventory(ui, pet, sprite_renderer);
            });
    }

    fn render_inventory(ui: &mut egui::Ui, pet: &CachedPet, sprite_renderer: &mut SpriteRenderer) {
        if pet.inventory.is_empty() {
            ui.label(RichText::new("no inventory data").small().weak());
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
            for item in &pet.inventory {
                if item.item_id < 0 {
                    sprite_renderer.render_empty_slot(ui, TILE_SIZE);
                } else {
                    sprite_renderer.render_item_tile_sized(ui, item.item_id, None, &[], TILE_SIZE);
                }
            }
        });
    }

    /// Instance ids present in both pools whose resolved sprite differs.
    /// Compared by sprite only: the `name` field is a stale original hatch name
    /// and is unreliable. Ambiguous by design: either stale/mis-detected
    /// identity, or the server reusing instance ids across pools.
    fn cross_pool_conflicts(
        regular: &std::collections::HashMap<i32, CachedPet>,
        seasonal: &std::collections::HashMap<i32, CachedPet>,
    ) -> Vec<(i32, String, String)> {
        let mut out = Vec::new();
        for (id, r) in regular {
            if let Some(s) = seasonal.get(id) {
                let r_sprite = Self::resolved_sprite(r);
                let s_sprite = Self::resolved_sprite(s);
                if r_sprite != s_sprite {
                    let am = get_asset_manager();
                    let r_name = am.object_name(r_sprite).unwrap_or_else(|| r.name.clone());
                    let s_name = am.object_name(s_sprite).unwrap_or_else(|| s.name.clone());
                    out.push((
                        *id,
                        format!("{} ({})", r_name, r_sprite),
                        format!("{} ({})", s_name, s_sprite),
                    ));
                }
            }
        }
        out.sort_by_key(|(id, _, _)| *id);
        out
    }
}

impl Panel for PetYardPanel {
    fn show(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext) -> Vec<AppAction> {
        let cache = &ctx.account_data.characters;
        let regular = cache.get_pets(false).clone();
        let seasonal = cache.get_pets(true).clone();
        let chars = cache.characters.clone();

        ui.add_space(4.0);
        ui.heading("Pet Yard (Diagnostics)");
        ui.label(
            RichText::new(
                "Read-only view of every pet in the identity/inventory cache. \
                 Pet identity is account-wide; inventory is tracked per pool. \
                 Equip a pet in-game and watch its entry update live.",
            )
            .weak()
            .small(),
        );

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Filter:");
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("instance id, name or sprite id")
                    .desired_width(240.0),
            );
            if !self.filter.is_empty() && ui.button("✖").clicked() {
                self.filter.clear();
            }
        });

        let conflicts = Self::cross_pool_conflicts(&regular, &seasonal);
        if !conflicts.is_empty() {
            ui.add_space(6.0);
            egui::Frame::NONE
                .fill(Color32::from_rgb(60, 30, 30))
                .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(180, 80, 80)))
                .corner_radius(6.0)
                .inner_margin(8.0)
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(format!(
                            "⚠ {} instance id(s) resolve differently per pool",
                            conflicts.len()
                        ))
                        .color(Color32::from_rgb(255, 200, 140))
                        .strong(),
                    );
                    ui.label(
                        RichText::new(
                            "The same instance id maps to a different sprite in the regular vs \
                             seasonal pool. If pets are account-wide, one side is stale/mis-detected; \
                             if the server reuses ids across pools, this is expected.",
                        )
                        .small()
                        .weak(),
                    );
                    for (id, r, s) in &conflicts {
                        ui.label(
                            RichText::new(format!("#{}: regular = {}  |  seasonal = {}", id, r, s))
                                .small(),
                        );
                    }
                });
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.columns(2, |cols| {
                Self::render_pool(
                    &mut cols[0],
                    "Regular",
                    &regular,
                    &chars,
                    false,
                    self,
                    ctx.sprite_renderer,
                );
                Self::render_pool(
                    &mut cols[1],
                    "Seasonal",
                    &seasonal,
                    &chars,
                    true,
                    self,
                    ctx.sprite_renderer,
                );
            });
        });

        Vec::new()
    }
}

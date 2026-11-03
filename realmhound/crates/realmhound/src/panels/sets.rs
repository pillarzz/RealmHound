//! Current, account-scoped ownership of the bundled four-piece sets.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

use eframe::egui::{self, Color32, RichText, ScrollArea};
use realmhound_core::{
    api::character::CharacterClass, assets::get_asset_manager, vault::AccountData,
};
use serde_json::Value;

use super::{ActiveTab, AppAction, Panel, PanelContext};
use crate::shadcn_ui::Shadcn;

const SETS: &str = include_str!("../../../../../data/rotmg-sets.json");
const SET_META: &str = include_str!("../../../../../data/rotmg-sets-sources.json");
const SOURCES: &str = include_str!("../../../../../data/rotmg-item-sources.json");
const IDS: &str = include_str!("../../../../../data/rotmg-set-item-ids.json");
const SLOTS: [&str; 4] = ["Weapon", "Ability", "Armor", "Jewelry"];

#[derive(Clone)]
struct Piece {
    name: String,
    id: Option<i32>,
    issue: Option<String>,
    source: Value,
}

#[derive(Clone)]
struct Set {
    id: String,
    name: String,
    slots: [Vec<Piece>; 4],
    classes: Vec<&'static str>,
}

#[derive(Default, Clone)]
struct Ownership {
    locations: Vec<OwnedLocation>,
}

#[derive(Clone)]
struct OwnedLocation {
    label: String,
    seasonal: bool,
}

impl Ownership {
    fn in_scope(&self, scope: InventoryScope) -> impl Iterator<Item = &str> {
        self.locations
            .iter()
            .filter(move |location| match scope {
                InventoryScope::Both => true,
                InventoryScope::Regular => !location.seasonal,
                InventoryScope::Seasonal => location.seasonal,
            })
            .map(|location| location.label.as_str())
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum InventoryScope {
    #[default]
    Both,
    Regular,
    Seasonal,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum SetSort {
    #[default]
    MostOwned,
    LeastOwned,
    Name,
}

#[derive(Default)]
pub struct SetsPanel {
    sets: Vec<Set>,
    categories: BTreeMap<String, String>,
    selected_categories: BTreeSet<String>,
    ownership: HashMap<i32, Ownership>,
    issues: Vec<String>,
    epoch: Option<u64>,
    search: String,
    class: String,
    completion: usize,
    owned_count: Option<usize>,
    sort: SetSort,
    inventory_scope: InventoryScope,
    expanded: BTreeSet<String>,
    loaded: bool,
    complete_coverage: bool,
    regular_coverage: bool,
    seasonal_coverage: bool,
}

fn normalized(s: &str) -> String {
    s.replace(['\u{2018}', '\u{2019}', '\u{02bc}'], "'")
}

fn stable_id(s: &str) -> String {
    s.to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn resolve(name: &str, ids: &Value) -> (Option<i32>, Option<String>) {
    let assets = get_asset_manager();
    let target = normalized(name);
    let Some(id) = ids[name].as_i64().and_then(|v| i32::try_from(v).ok()) else {
        return (
            None,
            Some(format!("{name}: absent from reviewed item ID mapping")),
        );
    };
    match assets.get_object(id) {
        Some(o)
            if o.class == "Equipment"
                && (normalized(o.name()) == target || normalized(&o.id_name) == target) =>
        {
            (Some(id), None)
        }
        Some(o) => (
            None,
            Some(format!(
                "{name}: mapped ID {id} is {} ({}) in installed assets",
                o.name(),
                o.class
            )),
        ),
        None => (
            None,
            Some(format!(
                "{name}: mapped ID {id} absent from installed game assets"
            )),
        ),
    }
}

fn classes_for(ability: &[Piece]) -> Vec<&'static str> {
    let assets = get_asset_manager();
    let mut names = BTreeSet::new();
    for piece in ability {
        let Some(id) = piece.id else { continue };
        let class = match assets.item_slot_type(id) {
            4 => "Priest",
            5 => "Knight",
            11 => "Wizard",
            12 => "Paladin",
            13 => "Rogue",
            15 => "Archer",
            16 => "Warrior",
            18 => "Assassin",
            19 => "Necromancer",
            20 => "Huntress",
            21 => "Mystic",
            22 => "Trickster",
            23 => "Sorcerer",
            25 => "Ninja",
            27 => "Samurai",
            28 => "Bard",
            29 => "Summoner",
            30 => "Kensei",
            31 => "Druid",
            _ => continue,
        };
        names.insert(class);
    }
    names.into_iter().collect()
}

impl SetsPanel {
    fn render_filters(&mut self, ui: &mut egui::Ui, shadcn: &Shadcn) {
        shadcn.header_band(ui, shadcn.secondary_header_fill(), |ui| {
            shadcn.band_row_wrapped(ui, |ui| {
                ui.label("🔍");
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .desired_width(150.0)
                        .hint_text("Set or item name"),
                );
                if !self.search.is_empty() && shadcn.btn_small(ui, "×").clicked() {
                    self.search.clear();
                }
                shadcn.band_divider(ui);
                ui.label("Class:");
                egui::ComboBox::from_id_salt("sets_class")
                    .selected_text(if self.class.is_empty() {
                        "All classes"
                    } else {
                        &self.class
                    })
                    .width(115.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.class, String::new(), "All classes");
                        for class in [
                            "Rogue",
                            "Archer",
                            "Wizard",
                            "Priest",
                            "Warrior",
                            "Knight",
                            "Paladin",
                            "Assassin",
                            "Necromancer",
                            "Huntress",
                            "Mystic",
                            "Trickster",
                            "Sorcerer",
                            "Ninja",
                            "Samurai",
                            "Bard",
                            "Summoner",
                            "Kensei",
                            "Druid",
                        ] {
                            ui.selectable_value(&mut self.class, class.into(), class);
                        }
                    });
                shadcn.band_divider(ui);
                ui.label("Inventory:");
                egui::ComboBox::from_id_salt("sets_inventory_scope")
                    .selected_text(match self.inventory_scope {
                        InventoryScope::Both => "All account",
                        InventoryScope::Regular => "Regular",
                        InventoryScope::Seasonal => "Seasonal",
                    })
                    .width(90.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.inventory_scope,
                            InventoryScope::Both,
                            "All account",
                        );
                        ui.selectable_value(
                            &mut self.inventory_scope,
                            InventoryScope::Regular,
                            "Regular",
                        );
                        ui.selectable_value(
                            &mut self.inventory_scope,
                            InventoryScope::Seasonal,
                            "Seasonal",
                        );
                    });
                shadcn.band_divider(ui);
                ui.label("Owned:");
                egui::ComboBox::from_id_salt("sets_owned_count")
                    .selected_text(self.owned_count.map_or("Any".into(), |n| format!("{n}/4")))
                    .width(55.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.owned_count, None, "Any");
                        for count in 0..=4 {
                            ui.selectable_value(
                                &mut self.owned_count,
                                Some(count),
                                format!("{count}/4"),
                            );
                        }
                    });
                shadcn.band_divider(ui);
                ui.label("Status:");
                egui::ComboBox::from_id_salt("sets_completion")
                    .selected_text(
                        ["All", "Complete", "Incomplete", "One missing"][self.completion],
                    )
                    .width(105.0)
                    .show_ui(ui, |ui| {
                        for (i, name) in ["All", "Complete", "Incomplete", "One missing"]
                            .iter()
                            .enumerate()
                        {
                            ui.selectable_value(&mut self.completion, i, *name);
                        }
                    });
                shadcn.band_divider(ui);
                ui.label("Sort:");
                egui::ComboBox::from_id_salt("sets_sort")
                    .selected_text(match self.sort {
                        SetSort::MostOwned => "Most complete",
                        SetSort::LeastOwned => "Least complete",
                        SetSort::Name => "Name A–Z",
                    })
                    .width(120.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.sort, SetSort::MostOwned, "Most complete");
                        ui.selectable_value(&mut self.sort, SetSort::LeastOwned, "Least complete");
                        ui.selectable_value(&mut self.sort, SetSort::Name, "Name A–Z");
                    });
            });
        });
        shadcn.header_band_stacked_divided(ui, shadcn.secondary_header_fill(), |ui| {
            shadcn.band_row_wrapped(ui, |ui| {
                ui.label("Missing-piece sources:");
                egui::ComboBox::from_id_salt("sets_location_categories")
                    .selected_text(if self.selected_categories.is_empty() {
                        "All categories".into()
                    } else {
                        format!("{} selected", self.selected_categories.len())
                    })
                    .width(130.0)
                    .show_ui(ui, |ui| {
                        for (id, label) in &self.categories {
                            let mut selected = self.selected_categories.contains(id);
                            if ui.checkbox(&mut selected, label).changed() {
                                if selected {
                                    self.selected_categories.insert(id.clone());
                                } else {
                                    self.selected_categories.remove(id);
                                }
                            }
                        }
                    });
                if !self.selected_categories.is_empty() {
                    if shadcn.btn_small(ui, "Clear sources").clicked() {
                        self.selected_categories.clear();
                    }
                    for id in self.selected_categories.clone() {
                        if let Some(label) = self.categories.get(&id) {
                            if shadcn.btn_small(ui, format!("{label} ×")).clicked() {
                                self.selected_categories.remove(&id);
                            }
                        }
                    }
                }
            });
        });
    }

    fn load(&mut self) {
        let Ok(raw_sets) = serde_json::from_str::<BTreeMap<String, Vec<String>>>(SETS) else {
            self.issues.push("Bundled set JSON is invalid".into());
            return;
        };
        let Ok(meta) = serde_json::from_str::<Value>(SET_META) else {
            self.issues.push("Bundled set metadata is invalid".into());
            return;
        };
        let Ok(sources) = serde_json::from_str::<Value>(SOURCES) else {
            self.issues.push("Bundled source JSON is invalid".into());
            return;
        };
        let Ok(ids) = serde_json::from_str::<Value>(IDS) else {
            self.issues
                .push("Bundled item ID mapping is invalid".into());
            return;
        };
        if let Some(map) = sources["location_categories"].as_object() {
            for (id, label) in map {
                if let Some(label) = label.as_str() {
                    self.categories.insert(id.clone(), label.into());
                }
            }
        }
        let alternate = meta["alternate_ring_entries"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        for (name, items) in &raw_sets {
            if alternate.contains_key(name) {
                continue;
            }
            if items.len() != 4 {
                self.issues.push(format!(
                    "{name}: expected four items, found {}",
                    items.len()
                ));
                continue;
            }
            let mut slots: [Vec<Piece>; 4] = std::array::from_fn(|_| Vec::new());
            for (slot, item_name) in items.iter().enumerate() {
                let (id, issue) = resolve(item_name, &ids);
                if let Some(issue) = &issue {
                    self.issues
                        .push(format!("{name} / {}: {issue}", SLOTS[slot]));
                }
                slots[slot].push(Piece {
                    name: item_name.clone(),
                    id,
                    issue,
                    source: sources["items"][item_name].clone(),
                });
            }
            for (variant, base) in &alternate {
                if base.as_str() != Some(name) {
                    continue;
                }
                if let Some(other) = raw_sets.get(variant) {
                    if other.len() == 4 && other[..3] == items[..3] && other[3] != items[3] {
                        let item_name = &other[3];
                        let (id, issue) = resolve(item_name, &ids);
                        if let Some(issue) = &issue {
                            self.issues
                                .push(format!("{name} / alternate Jewelry: {issue}"));
                        }
                        slots[3].push(Piece {
                            name: item_name.clone(),
                            id,
                            issue,
                            source: sources["items"][item_name].clone(),
                        });
                    } else {
                        self.issues.push(format!(
                            "{variant}: alternate entry differs outside jewelry"
                        ));
                    }
                }
            }
            let classes = classes_for(&slots[1]);
            self.sets.push(Set {
                id: stable_id(name),
                name: name.clone(),
                slots,
                classes,
            });
        }
        self.loaded = true;
    }

    fn rebuild_ownership(&mut self, account: &AccountData) {
        self.ownership.clear();
        let fresh = |time: Option<SystemTime>| {
            time.is_some_and(|t| {
                t.elapsed()
                    .is_ok_and(|age| age <= Duration::from_secs(24 * 60 * 60))
            })
        };
        self.regular_coverage = fresh(account.last_api_update)
            && fresh(account.regular_vault.last_updated)
            && !account.regular_vault.pending_update;
        self.seasonal_coverage = fresh(account.last_api_update)
            && fresh(account.seasonal_vault.last_updated)
            && !account.seasonal_vault.pending_update;
        self.complete_coverage = self.regular_coverage && self.seasonal_coverage;
        for ch in &account.characters.characters {
            let label = account
                .characters
                .custom_labels
                .get(&ch.char_id)
                .cloned()
                .unwrap_or_else(|| {
                    format!(
                        "{} #{}",
                        CharacterClass::from_id(ch.class_id).name(),
                        ch.char_id
                    )
                });
            for (where_, items) in [
                ("Equipped", &ch.equipment),
                ("Inventory", &ch.inventory),
                ("Backpack", &ch.backpack),
                ("Backpack extender", &ch.backpack_ext),
                ("Belt", &ch.belt),
            ] {
                for item in items {
                    if item.item_id > 0 {
                        self.ownership
                            .entry(item.item_id)
                            .or_default()
                            .locations
                            .push(OwnedLocation {
                                label: format!("{label} · {where_}"),
                                seasonal: ch.seasonal,
                            });
                    }
                }
            }
        }
        for (season, pets) in [
            ("Regular", &account.characters.regular_pets),
            ("Seasonal", &account.characters.seasonal_pets),
        ] {
            for pet in pets.values() {
                for item in &pet.inventory {
                    if item.item_id > 0 {
                        self.ownership
                            .entry(item.item_id)
                            .or_default()
                            .locations
                            .push(OwnedLocation {
                                label: format!("{season} pet {} · Inventory", pet.name),
                                seasonal: season == "Seasonal",
                            });
                    }
                }
            }
        }
        for (season, vault) in [
            ("Regular", &account.regular_vault),
            ("Seasonal", &account.seasonal_vault),
        ] {
            for (where_, items) in [
                ("Vault", &vault.vault_items),
                ("Materials", &vault.material_items),
                ("Gift Chest", &vault.gift_items),
                ("Potion Rack", &vault.potion_items),
                ("Spoils", &vault.spoils_items),
            ] {
                for item in items {
                    if item.item_id > 0 {
                        self.ownership
                            .entry(item.item_id)
                            .or_default()
                            .locations
                            .push(OwnedLocation {
                                label: format!("{season} {where_}"),
                                seasonal: season == "Seasonal",
                            });
                    }
                }
            }
        }
    }

    fn owned(&self, slot: &[Piece]) -> usize {
        slot.iter()
            .filter_map(|p| p.id)
            .map(|id| {
                self.ownership
                    .get(&id)
                    .map_or(0, |o| o.in_scope(self.inventory_scope).count())
            })
            .sum()
    }

    fn coverage_known(&self) -> bool {
        match self.inventory_scope {
            InventoryScope::Both => self.complete_coverage,
            InventoryScope::Regular => self.regular_coverage,
            InventoryScope::Seasonal => self.seasonal_coverage,
        }
    }

    fn slot_state(&self, slot: &[Piece]) -> &'static str {
        if self.owned(slot) > 0 {
            "Owned"
        } else if slot.iter().any(|p| p.id.is_none()) || !self.coverage_known() {
            "Unknown"
        } else {
            "Missing"
        }
    }

    fn category_match(&self, slot: &[Piece]) -> bool {
        slot.iter().any(|p| {
            p.source["drops_from"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|drop| {
                    drop["location_categories"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|c| {
                            c.as_str()
                                .is_some_and(|id| self.selected_categories.contains(id))
                        })
                })
        })
    }

    fn matches(&self, set: &Set) -> bool {
        let query = self.search.trim().to_lowercase();
        if !query.is_empty()
            && !set.name.to_lowercase().contains(&query)
            && !set
                .slots
                .iter()
                .flatten()
                .any(|p| p.name.to_lowercase().contains(&query))
        {
            return false;
        }
        if !self.class.is_empty() && !set.classes.contains(&self.class.as_str()) {
            return false;
        }
        let owned = set.slots.iter().filter(|s| self.owned(s) > 0).count();
        if self.owned_count.is_some_and(|count| count != owned) {
            return false;
        }
        let missing = set
            .slots
            .iter()
            .filter(|s| self.slot_state(s) == "Missing")
            .count();
        if match self.completion {
            1 => owned != 4,
            2 => owned == 4,
            3 => owned != 3 || missing != 1,
            _ => false,
        } {
            return false;
        }
        self.selected_categories.is_empty()
            || set.slots.iter().any(|s| {
                let relevant = if owned == 4 {
                    true
                } else {
                    self.slot_state(s) == "Missing"
                };
                relevant && self.category_match(s)
            })
    }

    fn sort_rows(&self, rows: &mut [Set]) {
        rows.sort_by(|a, b| {
            let owned = |set: &Set| set.slots.iter().filter(|slot| self.owned(slot) > 0).count();
            match self.sort {
                SetSort::MostOwned => owned(b).cmp(&owned(a)),
                SetSort::LeastOwned => owned(a).cmp(&owned(b)),
                SetSort::Name => std::cmp::Ordering::Equal,
            }
            .then_with(|| a.name.cmp(&b.name))
        });
    }
}

impl Panel for SetsPanel {
    fn show(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext) -> Vec<AppAction> {
        let mut actions = Vec::new();
        if !self.loaded {
            self.load();
        }
        if self.epoch != Some(ctx.account_epoch) {
            self.rebuild_ownership(ctx.account_data);
            self.epoch = Some(ctx.account_epoch);
        }
        ui.heading("Sets");
        ui.label("Current known account ownership · sources are wiki-documented, not guaranteed current availability");
        let age = |time: Option<std::time::SystemTime>| {
            time.and_then(|t| t.elapsed().ok())
                .map(|d| format!("{}h ago", d.as_secs() / 3600))
                .unwrap_or_else(|| "not observed".into())
        };
        ui.label(format!(
            "Snapshot age: characters {} · regular vault {} · seasonal vault {}",
            age(ctx.account_data.last_api_update),
            age(ctx.account_data.regular_vault.last_updated),
            age(ctx.account_data.seasonal_vault.last_updated)
        ));
        if !self.coverage_known() {
            ui.colored_label(Color32::YELLOW, "Partial or stale inventory (over 24 hours): unobserved storage may contain other pieces. Unconfirmed slots are unknown.");
        }
        if !self.issues.is_empty() {
            ui.collapsing(
                format!("{} unresolved data diagnostics", self.issues.len()),
                |ui| {
                    for issue in &self.issues {
                        ui.label(issue);
                    }
                },
            );
        }
        self.render_filters(ui, ctx.shadcn);
        let mut rows: Vec<_> = self
            .sets
            .iter()
            .filter(|s| self.matches(s))
            .cloned()
            .collect();
        self.sort_rows(&mut rows);
        ui.label(format!("{} of {} sets", rows.len(), self.sets.len()));
        if rows.is_empty() {
            super::empty_state(
                ui,
                "No matching sets",
                Some("Try changing the search or filters."),
            );
        }
        ScrollArea::both().show(ui, |ui| {
            const PIECE_WIDTH: f32 = 72.0;
            const ROW_HEIGHT: f32 = 54.0;
            let name_width =
                (ui.available_width() - 4.0 * PIECE_WIDTH - 5.0 * ui.spacing().item_spacing.x)
                    .max(220.0);
            ui.horizontal(|ui| {
                ui.add_sized(
                    [name_width, 20.0],
                    egui::Label::new(RichText::new("Set · owned / 4").strong()),
                );
                for slot in SLOTS {
                    ui.add_sized(
                        [PIECE_WIDTH, 20.0],
                        egui::Label::new(RichText::new(slot).strong()),
                    );
                }
            });
            ctx.shadcn.full_width_separator(ui);
            for set in rows {
                let owned = set.slots.iter().filter(|s| self.owned(s) > 0).count();
                let is_open = self.expanded.contains(&set.id);
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(name_width, ROW_HEIGHT),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.horizontal(|ui| {
                                if ctx
                                    .shadcn
                                    .btn_small(ui, if is_open { "▾" } else { "▸" })
                                    .clicked()
                                {
                                    if is_open {
                                        self.expanded.remove(&set.id);
                                    } else {
                                        self.expanded.insert(set.id.clone());
                                    }
                                }
                                ui.add_sized(
                                    [name_width - 38.0, 20.0],
                                    egui::Label::new(RichText::new(&set.name).strong()).truncate(),
                                )
                                .on_hover_text(&set.name);
                            });
                            let class_label = if set.classes.is_empty() {
                                "Class unknown".into()
                            } else {
                                set.classes.join(", ")
                            };
                            ui.add_sized(
                                [name_width, 18.0],
                                egui::Label::new(format!("{owned}/4 · {class_label}")).truncate(),
                            )
                            .on_hover_text(class_label);
                        },
                    );
                    for slot in &set.slots {
                        let state = self.slot_state(slot);
                        ui.allocate_ui_with_layout(
                            egui::vec2(PIECE_WIDTH, ROW_HEIGHT),
                            egui::Layout::top_down(egui::Align::Center),
                            |ui| {
                                if let Some(id) = slot.first().and_then(|p| p.id) {
                                    ctx.sprite_renderer
                                        .render_item_tile_sized(ui, id, None, &[], 30.0)
                                        .on_hover_text(state);
                                } else {
                                    ui.add_sized([30.0, 30.0], egui::Label::new("?"))
                                        .on_hover_text(state);
                                }
                                ui.label(
                                    RichText::new(match state {
                                        "Owned" => "✓ Owned",
                                        "Missing" => "– Missing",
                                        _ => "? Unknown",
                                    })
                                    .size(10.0)
                                    .color(match state {
                                        "Owned" => Color32::GREEN,
                                        "Missing" => Color32::RED,
                                        _ => Color32::YELLOW,
                                    }),
                                );
                            },
                        );
                    }
                });
                if is_open {
                    for (i, slot) in set.slots.iter().enumerate() {
                        let state = self.slot_state(slot);
                        ui.indent((set.id.as_str(), i), |ui| {
                            ui.label(format!(
                                "{} · {state}{}",
                                SLOTS[i],
                                if state == "Missing"
                                    && !self.selected_categories.is_empty()
                                    && self.category_match(slot)
                                {
                                    " · matches selected categories"
                                } else {
                                    ""
                                }
                            ));
                            for piece in slot {
                                ui.horizontal(|ui| {
                                    if let Some(id) = piece.id {
                                        ctx.sprite_renderer.render_item_tile_sized(
                                            ui,
                                            id,
                                            Some(&piece.name),
                                            &[],
                                            32.0,
                                        );
                                    }
                                    ui.label(&piece.name);
                                    if let Some(issue) = &piece.issue {
                                        ui.colored_label(Color32::YELLOW, issue);
                                    }
                                });
                                if let Some(id) = piece.id {
                                    if let Some(own) = self.ownership.get(&id) {
                                        let locations =
                                            own.in_scope(self.inventory_scope).collect::<Vec<_>>();
                                        if !locations.is_empty() {
                                            ui.label(format!(
                                                "{} owned · {}",
                                                locations.len(),
                                                locations.join(", ")
                                            ));
                                            if ui.small_button("Browse in Treasury").clicked() {
                                                actions.push(AppAction::SwitchTab(
                                                    ActiveTab::Treasury,
                                                ));
                                            }
                                        }
                                    }
                                }
                                source_details(ui, &piece.source, &self.categories);
                            }
                        });
                    }
                }
                ui.separator();
            }
        });
        actions
    }
}

fn source_details(ui: &mut egui::Ui, source: &Value, categories: &BTreeMap<String, String>) {
    if source.is_null() {
        ui.label("Wiki source information unavailable");
        return;
    }
    let status = source["status"].as_str().unwrap_or("unknown");
    if !matches!(status, "documented" | "documented_set_page") {
        ui.label(format!(
            "Wiki source information unavailable or undocumented ({status})"
        ));
    }
    let drops = source["drops_from"].as_array();
    let methods = source["obtained_through"].as_array();
    let summary = format!(
        "Wiki sources: {} drop source(s), {} other method(s)",
        drops.map_or(0, Vec::len),
        methods.map_or(0, Vec::len)
    );
    ui.collapsing(summary, |ui| {
        if let Some(url) = source["wiki_url"].as_str() { ui.hyperlink_to("Item wiki", url); }
        if let Some(url) = source["acquisition_reference"].as_str() { ui.hyperlink_to("Set page acquisition reference", url); }
        if let Some(text) = source["drops_from_text"].as_str() { ui.label(format!("Drop conditions: {text}")); }
        if let Some(text) = source["obtained_through_text"].as_str() { ui.label(format!("Other acquisition: {text}")); }
        for drop in drops.into_iter().flatten() {
            let name = drop["name"].as_str().unwrap_or("Unknown drop source");
            if let Some(url) = drop["wiki_url"].as_str() { ui.hyperlink_to(format!("Drop from {name}"), url); }
            else { ui.label(format!("Drop from {name}")); }
            if let Some(locations) = drop["source_locations"].as_array() { for location in locations {
                let where_ = location["name"].as_str().unwrap_or("Unknown location");
                let labels = location["location_categories"].as_array().into_iter().flatten()
                    .filter_map(Value::as_str).filter_map(|id| categories.get(id))
                    .cloned().collect::<Vec<_>>().join(", ");
                if let Some(url) = location["wiki_url"].as_str() { ui.hyperlink_to(format!("  Source entity documented in {where_} ({labels}); item drop here is unverified"), url); }
                else { ui.label(format!("  Source entity documented in {where_} ({labels}); item drop here is unverified")); }
            }}
            if drop["location_status"].as_str() != Some("documented") { ui.label("  Source location undocumented or uncertain"); }
        }
        for method in methods.into_iter().flatten() { if let Some(name) = method["name"].as_str() {
            if let Some(url) = method["wiki_url"].as_str() { ui.hyperlink_to(format!("Other method: {name}"), url); }
            else { ui.label(format!("Other method: {name}")); }
        } }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use realmhound_core::vault::{CachedCharacter, CharacterItem, LiveVaultItem};
    use std::time::SystemTime;

    fn piece(id: Option<i32>, source: Value) -> Piece {
        Piece {
            name: format!("Item {id:?}"),
            id,
            issue: None,
            source,
        }
    }
    fn sample_set() -> Set {
        Set {
            id: "sample".into(),
            name: "Sample Set".into(),
            classes: vec!["Priest"],
            slots: [
                vec![piece(Some(10), Value::Null)],
                vec![piece(Some(11), Value::Null)],
                vec![piece(Some(12), Value::Null)],
                vec![piece(Some(13), Value::Null)],
            ],
        }
    }
    fn covered() -> AccountData {
        let mut a = AccountData::new();
        a.last_api_update = Some(SystemTime::now());
        a.regular_vault.last_updated = Some(SystemTime::now());
        a.seasonal_vault.last_updated = Some(SystemTime::now());
        a
    }
    fn recorded(seasonal: bool) -> Ownership {
        Ownership {
            locations: vec![OwnedLocation {
                label: "Vault".into(),
                seasonal,
            }],
        }
    }

    #[test]
    fn ownership_spans_equipment_backpack_vault_and_gifts_and_updates() {
        let mut account = covered();
        let mut ch = CachedCharacter {
            char_id: 7,
            class_id: 784,
            ..Default::default()
        };
        ch.equipment.push(CharacterItem::new(10));
        ch.backpack.push(CharacterItem::new(11));
        account.characters.characters.push(ch);
        account
            .regular_vault
            .vault_items
            .push(LiveVaultItem::new(12));
        account
            .seasonal_vault
            .gift_items
            .push(LiveVaultItem::new(13));
        let mut panel = SetsPanel::default();
        panel.rebuild_ownership(&account);
        let set = sample_set();
        assert!(set
            .slots
            .iter()
            .all(|slot| panel.slot_state(slot) == "Owned"));
        assert!(panel.ownership[&10].locations[0]
            .label
            .contains("Priest #7 · Equipped"));
        assert!(panel.ownership[&13].locations[0]
            .label
            .contains("Seasonal Gift Chest"));
        account.characters.characters[0].backpack.clear();
        panel.rebuild_ownership(&account);
        assert_eq!(panel.slot_state(&set.slots[1]), "Missing");
        assert!(!panel.ownership.contains_key(&11));
        panel.rebuild_ownership(&covered());
        assert!(panel.ownership.is_empty()); // account isolation on snapshot replacement
    }

    #[test]
    fn duplicate_and_shared_items_count_once_per_slot() {
        let mut account = covered();
        account
            .regular_vault
            .vault_items
            .extend([LiveVaultItem::new(10), LiveVaultItem::new(10)]);
        let mut panel = SetsPanel::default();
        panel.rebuild_ownership(&account);
        let a = sample_set();
        let mut b = sample_set();
        b.id = "other".into();
        assert_eq!(panel.owned(&a.slots[0]), 2);
        assert_eq!(a.slots.iter().filter(|s| panel.owned(s) > 0).count(), 1);
        assert_eq!(b.slots.iter().filter(|s| panel.owned(s) > 0).count(), 1);
    }

    #[test]
    fn seasonal_scope_changes_counts_locations_and_coverage() {
        let mut account = covered();
        let mut seasonal_ch = CachedCharacter {
            char_id: 8,
            class_id: 784,
            seasonal: true,
            ..Default::default()
        };
        seasonal_ch.equipment.push(CharacterItem::new(10));
        account.characters.characters.push(seasonal_ch);
        account
            .regular_vault
            .vault_items
            .push(LiveVaultItem::new(10));
        account
            .seasonal_vault
            .gift_items
            .push(LiveVaultItem::new(11));
        let mut panel = SetsPanel::default();
        panel.rebuild_ownership(&account);
        let set = sample_set();
        assert_eq!(panel.owned(&set.slots[0]), 2);
        panel.inventory_scope = InventoryScope::Seasonal;
        assert_eq!(panel.owned(&set.slots[0]), 1);
        assert_eq!(panel.owned(&set.slots[1]), 1);
        assert!(panel.ownership[&10]
            .in_scope(panel.inventory_scope)
            .all(|where_| where_.contains("Priest #8")));
        panel.inventory_scope = InventoryScope::Regular;
        assert_eq!(panel.owned(&set.slots[0]), 1);
        assert_eq!(panel.owned(&set.slots[1]), 0);
        assert!(panel.ownership[&10]
            .in_scope(panel.inventory_scope)
            .all(|where_| where_.contains("Regular Vault")));
        account.seasonal_vault.last_updated = None;
        panel.rebuild_ownership(&account);
        assert!(panel.coverage_known()); // regular coverage remains usable
        panel.inventory_scope = InventoryScope::Seasonal;
        assert!(!panel.coverage_known());
        assert_eq!(panel.slot_state(&set.slots[2]), "Unknown");
        assert_eq!(panel.slot_state(&set.slots[0]), "Owned");
    }

    #[test]
    fn alternatives_unresolved_and_partial_coverage() {
        let mut panel = SetsPanel::default();
        let slot = vec![piece(Some(13), Value::Null), piece(Some(14), Value::Null)];
        assert_eq!(panel.slot_state(&slot), "Unknown");
        let mut account = covered();
        account
            .regular_vault
            .gift_items
            .push(LiveVaultItem::new(14));
        panel.rebuild_ownership(&account);
        assert_eq!(panel.slot_state(&slot), "Owned");
        assert_eq!(panel.owned(&slot), 1);
        assert_eq!(panel.slot_state(&[piece(None, Value::Null)]), "Unknown");
        assert_eq!(
            panel.slot_state(&[piece(Some(999), Value::Null)]),
            "Missing"
        );
        account.seasonal_vault.last_updated = None;
        panel.rebuild_ownership(&account);
        assert_eq!(
            panel.slot_state(&[piece(Some(999), Value::Null)]),
            "Unknown"
        );
        assert_eq!(panel.slot_state(&slot), "Owned");
        account.seasonal_vault.last_updated =
            Some(SystemTime::now() - Duration::from_secs(48 * 60 * 60));
        panel.rebuild_ownership(&account);
        assert_eq!(
            panel.slot_state(&[piece(Some(999), Value::Null)]),
            "Unknown"
        );
    }

    #[test]
    fn category_filter_uses_missing_slots_alternatives_and_combined_filters() {
        let realm = serde_json::json!({"drops_from": [{"name":"Boss", "location_categories":["realm_dungeons"]}]});
        let event = serde_json::json!({"drops_from": [{"name":"Other", "location_categories":["special_event_dungeons"]}]});
        let unknown = serde_json::json!({"drops_from": [{"name":"Unclassified", "location_categories":["unknown"]}]});
        let mut set = sample_set();
        set.slots[3] = vec![piece(Some(13), realm), piece(Some(14), event)];
        let mut panel = SetsPanel::default();
        panel.complete_coverage = true;
        for id in [10, 11, 12] {
            panel.ownership.insert(id, recorded(false));
        }
        panel.completion = 3;
        panel.class = "Priest".into();
        panel.search = "Sample".into();
        panel
            .selected_categories
            .insert("special_event_dungeons".into());
        assert!(panel.matches(&set));
        panel.selected_categories.clear();
        panel.selected_categories.insert("realm_dungeons".into());
        assert!(panel.matches(&set));
        panel.selected_categories.clear();
        panel.selected_categories.insert("unknown".into());
        assert!(!panel.matches(&set));
        set.slots[3].push(piece(Some(15), unknown));
        assert!(panel.matches(&set));
        panel.ownership.insert(13, recorded(false));
        panel.completion = 1;
        assert!(panel.matches(&set)); // complete sets inspect owned slots too
        panel.completion = 3;
        assert!(!panel.matches(&set));
        panel.completion = 0;
        panel.class = "Wizard".into();
        assert!(!panel.matches(&set));
    }

    #[test]
    fn bundled_alternates_and_distinct_legacy_names() {
        let sets: BTreeMap<String, Vec<String>> = serde_json::from_str(SETS).unwrap();
        let meta: Value = serde_json::from_str(SET_META).unwrap();
        for (variant, base) in meta["alternate_ring_entries"].as_object().unwrap() {
            let base = base.as_str().unwrap();
            assert_eq!(sets[variant][..3], sets[base][..3]);
            assert_ne!(sets[variant][3], sets[base][3]);
        }
        let ids: Value = serde_json::from_str(IDS).unwrap();
        assert_ne!(ids["Etherite Dagger"], ids["Legacy Etherite Dagger"]);
    }

    #[test]
    fn unavailable_and_conditional_sources_do_not_invent_locations() {
        let mut panel = SetsPanel::default();
        panel.selected_categories.insert("realm_dungeons".into());
        let unavailable = piece(Some(1), Value::Null);
        let historical = piece(
            Some(2),
            serde_json::json!({
                "status":"documented", "obtained_through_text":"Historical campaign reward",
                "obtained_through":[{"name":"Campaign"}], "drops_from":[]
            }),
        );
        assert!(!panel.category_match(&[unavailable, historical]));
        let appearance_only = piece(
            Some(3),
            serde_json::json!({
                "drops_from":[{"name":"Boss", "location_status":"documented",
                    "source_locations":[{"name":"A special encounter", "location_categories":["special_event_dungeons"]}],
                    "location_categories":["special_event_dungeons"]}]
            }),
        );
        assert!(!panel.category_match(&[appearance_only.clone()]));
        panel
            .selected_categories
            .insert("special_event_dungeons".into());
        assert!(panel.category_match(&[appearance_only]));
    }

    #[test]
    fn owned_count_filter_and_completeness_sort_are_deterministic() {
        let mut panel = SetsPanel::default();
        panel.complete_coverage = true;
        let make_set = |name: &str, base: i32| Set {
            id: stable_id(name),
            name: name.into(),
            classes: vec!["Priest"],
            slots: std::array::from_fn(|i| vec![piece(Some(base + i as i32), Value::Null)]),
        };
        let mut rows = vec![
            make_set("Alpha", 10),
            make_set("Beta", 20),
            make_set("Gamma", 30),
        ];
        for id in [10, 20, 21, 22, 30, 31, 32, 33] {
            panel.ownership.insert(id, recorded(false));
        }
        panel.owned_count = Some(3);
        assert!(!panel.matches(&rows[0]));
        assert!(panel.matches(&rows[1]));
        assert!(!panel.matches(&rows[2]));
        panel.class = "Wizard".into();
        assert!(!panel.matches(&rows[1]));
        panel.class.clear();
        panel.owned_count = None;
        panel.sort_rows(&mut rows);
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["Gamma", "Beta", "Alpha"]
        );
        panel.sort = SetSort::LeastOwned;
        panel.sort_rows(&mut rows);
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["Alpha", "Beta", "Gamma"]
        );
        panel.sort = SetSort::Name;
        panel.sort_rows(&mut rows);
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["Alpha", "Beta", "Gamma"]
        );
    }

    #[test]
    fn renders_fixture_snapshot_in_egui() {
        let mut panel = SetsPanel::default();
        panel.search = "Shaman Necromancer".into();
        let mut account = covered();
        let ids: Value = serde_json::from_str(IDS).unwrap();
        let fang = ids["Fang Necklace"].as_i64().unwrap() as i32;
        account
            .regular_vault
            .gift_items
            .push(LiveVaultItem::new(fang));
        let mut renderer = crate::rendering::SpriteRenderer::new();
        let shadcn = crate::shadcn_ui::Shadcn::default();
        let labels = HashMap::new();
        let token = None;
        let mission = crate::processing::MissionView::default();
        let egui = egui::Context::default();
        let _ = egui.run(egui::RawInput::default(), |egui| {
            egui::CentralPanel::default().show(egui, |ui| {
                let mut ctx = PanelContext {
                    sprite_renderer: &mut renderer,
                    access_token: &token,
                    token_captured_at: None,
                    labels: &labels,
                    loot_database: None,
                    combat_database: None,
                    account_data: &account,
                    account_epoch: 1,
                    shadcn: &shadcn,
                    client_launch_unix: 0,
                    account_verified: true,
                    mission_view: &mission,
                    live_char_id: None,
                    api_refresh_allowed: false,
                };
                panel.show(ui, &mut ctx);
            });
        });
        assert_eq!(panel.sets.len(), 108);
        assert!(panel.ownership.contains_key(&fang));
        if get_asset_manager().get_object(43381).is_some() {
            assert!(panel.issues.is_empty(), "{}", panel.issues.join("\n"));
        }
    }
}

//! Missions panel.
//!
//! A glance-first mission **table** styled after Combat History fight cards:
//! each mission is a rounded card filled with the fight-card colour, laid out in
//! aligned columns under a shared header row.
//!
//! Columns: `[icon]  Type  Status  Mission  Objective  →  Reward`.
//!
//! - Mission **name** is always fully visible (Combat-History title size); only
//!   the description lives in a tooltip.
//! - **Type** is an in-game-style badge (coloured rect, white text, black
//!   outline): gold Regular, green Seasonal, red Crucible. The filter chips use
//!   the same badge look.
//! - **Status** (merged with repeatability): a coloured status line -- "In
//!   progress" (blue), "Claimable" (yellow), "Claimed" (green) -- with a reset
//!   line below it: repeatable missions on cooldown show a reload glyph +
//!   "Resets --:--"; repeatable missions still active show "Repeatable".
//!   One-time claimed missions are auto-hidden (toggle to reveal); repeatable
//!   claimed missions grey out and sink to the bottom on cooldown.
//! - **Objective**: a grey description subtitle, then an in-game-style progress
//!   bar per objective (icon + fill + `have/need`; blue in progress, gold when
//!   complete). AND groups of small-count (1-3) dungeons collapse to one compact
//!   horizontal portal row; other multi-choice missions get one bar per choice.
//! - **Reward**: a single line -- BXP first (white number + bold green "BXP"),
//!   then item groups joined by `+`; items inside a pick-one group joined by
//!   `or`. Stacked items show `x N` after the sprite.
//!
//! Data comes from the worker-side `MissionTracker` via the model snapshot.
//!
//! Deferred (needs a live payload): a unique in-game mission icon (uses the
//! reward sprite as a fallback) and the exact cooldown/reset countdown.

use std::collections::{HashMap, HashSet};

use eframe::egui::{
    self, Align, Color32, FontId, Layout, RichText, Sense, Stroke, StrokeKind, UiBuilder,
};
use realmhound_core::api::MissionState;
use realmhound_core::assets::{
    dungeon_difficulty, dungeons_in_difficulty_range, encounter_spawn_limited, get_asset_manager,
    get_dungeon_drops, get_dungeon_portal_map, normalize_train_sprite, BiomeSection, BiomeTier,
    BossGroup, CatalogEntry, DropCategory,
};
use realmhound_core::vault::AccountData;

use crate::panels::{empty_state, empty_state_warning, AppAction, Panel, PanelContext};
use crate::processing::{
    MissionCategory, MissionEntryView, ObjectiveKind, ObjectiveView, RewardGroup, RewardView,
    WornReqView,
};
use crate::rendering::sprite_renderer::SpriteRenderer;
use crate::rendering::EmbeddedIcon;
use crate::ui_ext::HoverTooltipExt;

const CATEGORIES: [MissionCategory; 4] = [
    MissionCategory::Regular,
    MissionCategory::Seasonal,
    MissionCategory::Crucible,
    MissionCategory::Event,
];

// Layout metrics.
const PAD_X: f32 = 10.0;
const PAD_Y: f32 = 6.0;
const CARD_GAP: f32 = 4.0;
const COL_GAP: f32 = 8.0;

const W_ICON: f32 = 40.0;
const W_TYPE: f32 = 84.0;
const W_STATUS: f32 = 108.0;
const W_NAME_MIN: f32 = 120.0;
const W_NAME_MAX: f32 = 264.0;
const NAME_GAP: f32 = 3.0;
const W_OBJ: f32 = 296.0;
const W_ARROW: f32 = 28.0;
const REWARD_MIN: f32 = 160.0;

const MISSION_ICON: f32 = 32.0;
const OBJ_ICON: f32 = 22.0;
/// Target height (logical px) for inline sprites inside hover tooltips. Matches
/// the crisp icons used in other tooltips (e.g. the Exaltations "Exalted in
/// dungeons" tip) so Missions tooltips read consistently.
const TIP_ICON: f32 = 20.0;
/// Horizontal gap between an objective's icon slot and its progress bar.
const OBJ_ICON_GAP: f32 = 5.0;
/// Left margin reserved before every progress bar (and matched by the objective
/// description text) so bars and text align on one vertical line whether or not
/// an objective has a portal/boss icon.
const OBJ_INDENT: f32 = OBJ_ICON + OBJ_ICON_GAP;
const BAR_H: f32 = 18.0;
const BAR_W: f32 = 168.0;
const REWARD_TILE: f32 = 26.0;
/// Extra horizontal padding added on each side of a "+" separator in the reward
/// line so grouped rewards read as slightly more spaced out (on top of the row's
/// 6px item spacing).
const REWARD_PLUS_PAD: f32 = 3.0;
const LINE_H: f32 = 26.0;
const DESC_ROW_H: f32 = 17.0;
const BASE_H: f32 = 40.0;
const NAME_SIZE: f32 = 17.0;

/// Sum of the fixed columns plus the gaps that precede the reward column, given
/// the (dynamic) mission-name column width.
fn fixed_span(name_w: f32, hide_name_icon: bool) -> f32 {
    if hide_name_icon {
        // Icon and name columns (and their gaps) dropped.
        W_TYPE + W_STATUS + W_OBJ + W_ARROW + COL_GAP * 4.0
    } else {
        W_ICON + W_TYPE + W_STATUS + name_w + W_OBJ + W_ARROW + COL_GAP * 5.0 + NAME_GAP
    }
}

#[derive(Default)]
pub struct MissionsPanel {
    /// Hide missions already claimed / permanently completed.
    hide_claimed: bool,
    /// Hide the mission icon and name columns (helps on smaller screens).
    hide_name_icon: bool,
    /// Mission uids the user temporarily hid.
    hidden: HashSet<i64>,
    /// User-defined drag order (mission uids). Missions not listed fall back to
    /// most-progressed order within their status section.
    user_order: Vec<i64>,
    /// Category labels toggled OFF (empty = show all).
    disabled_labels: HashSet<u8>,
    /// Status section ids collapsed by the user (0..4; empty = all expanded).
    collapsed: HashSet<u8>,
    /// Whether the Hidden section is expanded (collapsed by default).
    hidden_expanded: bool,
    /// Mission id currently being dragged (transient, not persisted).
    dragging: Option<i64>,
    /// Drop index within the dragged mission's section (transient).
    drop_index: Option<usize>,
    /// Set when persisted state changed this frame (triggers a settings save).
    dirty: bool,
    /// Live Feed Taskbar tracking settings (mirrors `LiveFeedSettings.taskbar`).
    /// Drives the per-card compass toggle and its persistence.
    taskbar: realmhound_core::settings::TaskbarSettings,
}

impl MissionsPanel {
    pub fn new() -> Self {
        Self {
            // Permanently-claimed one-time missions are redundant clutter; hide
            // them by default (the toggle reveals them).
            hide_claimed: true,
            ..Self::default()
        }
    }

    /// Apply persisted settings after construction.
    pub fn apply_settings(&mut self, s: &realmhound_core::settings::MissionsSettings) {
        self.hide_claimed = s.hide_claimed;
        self.hide_name_icon = s.hide_name_icon;
        self.hidden = s.hidden.iter().copied().collect();
        self.user_order = s.user_order.clone();
        self.collapsed = s.collapsed.iter().copied().collect();
    }

    /// Apply the Live Feed Taskbar settings. Call on startup and whenever the
    /// Taskbar settings change so the compass toggles stay in sync.
    pub fn apply_taskbar_settings(&mut self, t: &realmhound_core::settings::TaskbarSettings) {
        self.taskbar = t.clone();
    }

    /// Mission uids the user has hidden. These stay out of the Taskbar even when
    /// the master "Track missions" toggle is on.
    pub fn hidden_mission_ids(&self) -> &HashSet<i64> {
        &self.hidden
    }

    /// Snapshot the persisted settings for saving.
    fn to_settings(&self) -> realmhound_core::settings::MissionsSettings {
        realmhound_core::settings::MissionsSettings {
            hidden: self.hidden.iter().copied().collect(),
            user_order: self.user_order.clone(),
            collapsed: self.collapsed.iter().copied().collect(),
            hide_claimed: self.hide_claimed,
            hide_name_icon: self.hide_name_icon,
        }
    }

    /// Position of `id` in the user drag order, or `usize::MAX` if unranked.
    fn order_pos(&self, id: i64) -> usize {
        self.user_order
            .iter()
            .position(|&x| x == id)
            .unwrap_or(usize::MAX)
    }

    /// Re-rank `section_ids` to the given order, then persist. Only these ids'
    /// relative order matters (sections are grouped), so we drop them from the
    /// global order and re-append in the new sequence.
    fn set_section_order(&mut self, section_ids: &[i64]) {
        let set: HashSet<i64> = section_ids.iter().copied().collect();
        self.user_order.retain(|id| !set.contains(id));
        self.user_order.extend_from_slice(section_ids);
        self.dirty = true;
    }
}

/// Reward is ready to claim (eligible, not yet claimed). A locked mission is
/// never claimable, even if it carries stale completed progress.
fn is_claimable(e: &MissionEntryView) -> bool {
    matches!(e.state, MissionState::Claimable)
        || (e.complete && !matches!(e.state, MissionState::Claimed | MissionState::Locked))
}

/// Repeatable mission that has been claimed => waiting to reset (on cooldown).
pub(crate) fn on_cooldown(e: &MissionEntryView) -> bool {
    e.repeatable && e.state == MissionState::Claimed
}

pub(crate) fn completion_fraction(e: &MissionEntryView) -> f32 {
    let mut sum = 0.0;
    let mut n = 0;
    for o in &e.objectives {
        if o.need > 0 {
            sum += (o.have as f32 / o.need as f32).clamp(0.0, 1.0);
            n += 1;
        }
    }
    if n == 0 {
        0.0
    } else {
        sum / n as f32
    }
}

/// Current wall-clock as unix seconds.
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Compact countdown for the card ("3d 23h", "5h 12m", "12m", "<1m").
fn fmt_countdown_short(secs: i64) -> String {
    let s = secs.max(0);
    let d = s / 86_400;
    let h = (s % 86_400) / 3600;
    let m = (s % 3600) / 60;
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        "<1m".to_string()
    }
}

/// Verbose countdown for the tooltip ("3 days, 23 hours and 27 minutes").
fn fmt_countdown_long(secs: i64) -> String {
    let s = secs.max(0);
    let d = s / 86_400;
    let h = (s % 86_400) / 3600;
    let m = (s % 3600) / 60;
    let unit = |n: i64, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
    let mut parts = Vec::new();
    if d > 0 {
        parts.push(unit(d, "day"));
    }
    if h > 0 {
        parts.push(unit(h, "hour"));
    }
    parts.push(unit(m, "minute"));
    match parts.len() {
        1 => parts[0].clone(),
        _ => {
            let last = parts.pop().unwrap();
            format!("{} and {}", parts.join(", "), last)
        }
    }
}

/// Group an integer with spaces every three digits (Deca's BXP style).
fn space_thousands(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

/// Shorten battle-pass XP to the in-game thousands form, number only:
/// `6,000,000 -> "6 000"`, `4,500 -> "4.5"`. Sub-thousand amounts stay raw.
fn format_bxp_num(n: i64) -> String {
    if n.abs() < 1000 {
        return n.to_string();
    }
    let k = n / 1000;
    let rem = (n % 1000).abs();
    if rem == 0 {
        space_thousands(k)
    } else {
        format!("{}.{}", space_thousands(k), rem / 100)
    }
}

/// Draw a category badge (filled colour rect, white text, 1px black outline,
/// CAPS) into an allocated rect using the given sense. `bright` picks the full
/// colour + white text; otherwise a dimmed variant. Returns the click response.
fn draw_badge(
    ui: &mut egui::Ui,
    cat: MissionCategory,
    bright: bool,
    sense: Sense,
) -> egui::Response {
    let text = cat.label();
    let font = FontId::proportional(11.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE);
    let size = galley.size() + egui::vec2(10.0, 4.0);
    let (rect, resp) = ui.allocate_exact_size(size, sense);

    let (r, g, b) = cat.color();
    let fill = if bright {
        Color32::from_rgb(r, g, b)
    } else {
        Color32::from_rgb(r / 3, g / 3, b / 3)
    };
    ui.painter().rect_filled(rect, 3.0, fill);

    let center = rect.center();
    let outline = Color32::from_black_alpha(if bright { 220 } else { 120 });
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
    let fg = if bright {
        Color32::WHITE
    } else {
        Color32::from_gray(180)
    };
    ui.painter()
        .text(center, egui::Align2::CENTER_CENTER, text, font, fg);
    resp
}

/// In-game-style category badge on a card: gold Regular, green Seasonal, red
/// Crucible. Dimmed when the mission is greyed. Clickable -- toggles that
/// category's list filter, just like the header chips.
fn type_badge(ui: &mut egui::Ui, cat: MissionCategory, greyed: bool) -> egui::Response {
    draw_badge(ui, cat, !greyed, Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .hover_tip(format!("Click to show/hide {} missions", cat.label()))
}

/// Clickable filter chip using the same badge look, dimmed when toggled off.
fn filter_badge(ui: &mut egui::Ui, cat: MissionCategory, on: bool) -> egui::Response {
    draw_badge(ui, cat, on, Sense::click())
}

/// Live character appearance + type flags, resolved once per frame so the card
/// loop can render the sprite and a qualification indicator without holding an
/// `account_data` borrow.
#[derive(Clone, Copy)]
pub(crate) struct CurrentChar {
    /// Skin sprite id (falls back to `class_id` when no skin is equipped).
    skin_id: i32,
    /// Base class sprite id, used as a fallback if the skin sprite is missing.
    class_id: i32,
    tex1: u32,
    tex2: u32,
    seasonal: bool,
    crucible_active: bool,
    /// Equipment `SlotType`s currently equipped (weapon, ability, armor, ring);
    /// `0` for empty/unknown slots. Used to evaluate mission worn restrictions.
    equipped_slots: [i32; 4],
}

/// Whether the current character qualifies for a mission with the given
/// eligibility `participants` bitmask. `None` when the mask is unknown (e.g.
/// placeholder rows before definitions load, or an unsupported mask) so callers
/// draw no indicator.
///
/// When the mission carries `worn` restrictions the character must additionally
/// have an item of one of those slot types equipped (e.g. an Orb -> Mystic).
/// The pill stays dimmed until a matching equipped item is found.
fn mission_qualifies(participants: i32, worn: &[WornReqView], c: &CurrentChar) -> Option<bool> {
    let base = match participants {
        255 => Some(true),
        2 => Some(c.seasonal),
        4 => Some(c.crucible_active),
        _ => None,
    }?;
    if worn.is_empty() {
        return Some(base);
    }
    let worn_ok = worn.iter().any(|w| c.equipped_slots.contains(&w.slot_type));
    Some(base && worn_ok)
}

/// Paint a small check (qualifies) or X (does not) centred at `c`.
fn draw_qualifier_glyph(p: &egui::Painter, c: egui::Pos2, ok: bool, greyed: bool) {
    let base = if ok {
        Color32::from_rgb(90, 200, 90)
    } else {
        Color32::from_rgb(214, 74, 62)
    };
    let col = if greyed {
        Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), 120)
    } else {
        base
    };
    let stroke = Stroke::new(2.0_f32, col);
    if ok {
        p.line_segment(
            [c + egui::vec2(-4.0, 0.0), c + egui::vec2(-1.0, 3.5)],
            stroke,
        );
        p.line_segment(
            [c + egui::vec2(-1.0, 3.5), c + egui::vec2(5.0, -4.0)],
            stroke,
        );
    } else {
        p.line_segment(
            [c + egui::vec2(-4.0, -4.0), c + egui::vec2(4.0, 4.0)],
            stroke,
        );
        p.line_segment(
            [c + egui::vec2(-4.0, 4.0), c + egui::vec2(4.0, -4.0)],
            stroke,
        );
    }
}

/// Draw the live character's sprite (skin + dyes) followed by a qualification
/// check/X, rendered as one centred row beneath the mission tag.
fn draw_char_qualifier(
    ui: &mut egui::Ui,
    sr: &mut SpriteRenderer,
    c: &CurrentChar,
    participants: i32,
    worn: &[WornReqView],
    greyed: bool,
) {
    const SP: f32 = 22.0;
    const GLYPH_W: f32 = 14.0;
    const GAP: f32 = 3.0;
    let qualifies = mission_qualifies(participants, worn, c);
    let tip = |ok: bool| {
        if ok {
            "Your active character qualifies for this mission"
        } else {
            "Your active character does not qualify for this mission"
        }
    };
    // Allocate an exact-width row so the outer top-down(Center) layout centres
    // the glyph + sprite under the tag instead of left-aligning them.
    let row_w = if qualifies.is_some() {
        GLYPH_W + GAP + SP
    } else {
        SP
    };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(row_w, SP), Sense::hover());
    let mut rui = ui.new_child(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    rui.spacing_mut().item_spacing.x = GAP;
    if let Some(ok) = qualifies {
        let (irect, iresp) = rui.allocate_exact_size(egui::vec2(GLYPH_W, SP), Sense::hover());
        draw_qualifier_glyph(rui.painter(), irect.center(), ok, greyed);
        iresp.hover_tip(tip(ok));
    }
    let (srect, sresp) = rui.allocate_exact_size(egui::vec2(SP, SP), Sense::hover());
    let drawn = sr.draw_dyed_outlined_character_sprite(&rui, c.skin_id, srect, 6, c.tex1, c.tex2);
    if !drawn && c.class_id != c.skin_id {
        sr.draw_dyed_outlined_character_sprite(&rui, c.class_id, srect, 6, c.tex1, c.tex2);
    }
    if let Some(ok) = qualifies {
        sresp.hover_tip(tip(ok));
    }
}

/// Resolve the four equipped-item slot types (weapon, ability, armor, ring) of
/// a character; `0` for empty/unknown slots. Drives mission worn-restriction
/// eligibility (e.g. an equipped Orb qualifies a Mystic-only mission).
fn equipped_slot_types(c: &realmhound_core::vault::CachedCharacter) -> [i32; 4] {
    let am = get_asset_manager();
    let mut slots = [0i32; 4];
    for (i, slot) in slots.iter_mut().enumerate() {
        if let Some(item) = c.equipment.get(i) {
            if item.item_id >= 0 {
                *slot = am.item_slot_type(item.item_id);
            }
        }
    }
    slots
}

/// Resolve the live character (skin/dyes + seasonal/crucible flags) for the
/// Taskbar so its pills can show the sprite and eligibility just like the cards.
pub(crate) fn current_char_from(
    account_data: &AccountData,
    live_char_id: Option<i32>,
) -> Option<CurrentChar> {
    let id = live_char_id?;
    account_data.find_character(id).map(|c| CurrentChar {
        skin_id: if c.skin > 0 {
            c.skin
        } else {
            c.class_id as i32
        },
        class_id: c.class_id as i32,
        tex1: c.tex1,
        tex2: c.tex2,
        seasonal: c.seasonal,
        crucible_active: c.crucible_active,
        equipped_slots: equipped_slot_types(c),
    })
}

/// Whether the current character can make progress on a mission with the given
/// eligibility `participants` mask and worn-equipment restrictions.
///
/// Before a live character is resolved (e.g. right after launch, before the
/// first area change) we assume the least-privileged character -- non-seasonal,
/// non-crucible, no special ability equipped -- so restricted missions start
/// dimmed and only brighten once a qualifying character is confirmed. Defaulting
/// to bright instead produces a jarring "available -> locked" flip. Unknown
/// participant masks still count as eligible so ordinary pills stay bright.
pub(crate) fn mission_eligible(
    current_char: Option<CurrentChar>,
    participants: i32,
    worn: &[WornReqView],
) -> bool {
    let c = current_char.unwrap_or(CurrentChar {
        skin_id: 0,
        class_id: 0,
        tex1: 0,
        tex2: 0,
        seasonal: false,
        crucible_active: false,
        equipped_slots: [0; 4],
    });
    mission_qualifies(participants, worn, &c).unwrap_or(true)
}

/// Render a mission as a vertical hover card for the Live Feed Taskbar,
/// reproducing the Missions tab card visuals: type badge + character qualifier +
/// repeatable tag, the mission name, its objective progress bars (with "OR"/
/// "AND" connectors), and the reward line.
pub(crate) fn render_mission_tooltip(
    ui: &mut egui::Ui,
    sr: &mut SpriteRenderer,
    e: &MissionEntryView,
    current_char: Option<CurrentChar>,
    tooltip_objs: &[usize],
    choice: bool,
) {
    ui.spacing_mut().item_spacing.y = 6.0;

    // Dim the whole tooltip when the mission is currently inactive: on cooldown
    // (repeatable, awaiting reset) or the live character can't progress it.
    let on_cd = on_cooldown(e);
    let ineligible = !mission_eligible(current_char, e.participants, &e.worn_restriction);
    let greyed = on_cd || ineligible;
    if greyed {
        ui.multiply_opacity(0.55);
    }

    // Header: character qualifier + type badge + repeatable tag.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if let Some(c) = current_char {
            draw_char_qualifier(ui, sr, &c, e.participants, &e.worn_restriction, greyed);
        }
        draw_badge(ui, e.category, true, Sense::hover());
        if e.repeatable {
            let y = Color32::from_rgb(235, 205, 70);
            let rep = if on_cd {
                "Repeatable (On cooldown)"
            } else {
                "Repeatable"
            };
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                ui.label(RichText::new("⟳").size(13.0).color(y));
                ui.label(RichText::new(rep).size(11.0).color(y));
            });
        }
    });

    ui.label(
        RichText::new(&e.name)
            .size(NAME_SIZE)
            .strong()
            .color(Color32::WHITE),
    );

    if !e.desc.is_empty() {
        ui.label(
            RichText::new(&e.desc)
                .size(12.5)
                .color(Color32::from_gray(150)),
        );
    }

    // Explain a char-ineligible mission's participant restriction, italicised
    // alongside the worn ("Requires X equipped") line.
    if ineligible {
        let req = match e.participants {
            2 => Some("Requires a seasonal character to progress"),
            4 => Some("Requires a crucible character to progress"),
            _ => None,
        };
        if let Some(t) = req {
            ui.label(
                RichText::new(t)
                    .size(12.0)
                    .italics()
                    .color(Color32::from_rgb(235, 205, 70)),
            );
        }
    }

    if !e.worn_restriction.is_empty() {
        let names: Vec<&str> = e
            .worn_restriction
            .iter()
            .map(|w| w.display.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        if !names.is_empty() {
            ui.label(
                RichText::new(format!("Requires {} equipped", names.join(" or ")))
                    .size(12.0)
                    .italics()
                    .color(Color32::from_rgb(235, 205, 70)),
            );
        }
    }
    // just the progressed variant once a choice is decided). `choice` still
    // pending renders variants joined with `OR`.
    let mission_icon = mission_fallback_icon(e);
    let n = tooltip_objs.len();
    let connector = if choice { "OR" } else { "AND" };
    for (i, &oi) in tooltip_objs.iter().enumerate() {
        if let Some(obj) = e.objectives.get(oi) {
            let c = (n > 1 && i + 1 < n).then_some(connector);
            objective_line(ui, sr, obj, &mission_icon, false, c, false);
        }
    }

    if !e.reward_groups.is_empty() {
        ui.add_space(2.0);
        ui.label(
            RichText::new("Reward")
                .size(12.0)
                .strong()
                .color(Color32::from_gray(160)),
        );
        rewards_line(ui, sr, &e.reward_groups, false);
    }

    // Join the mission-card objective tooltips: each still-incomplete relevant
    // objective with spawn/drop or qualifying-dungeon detail gets its own
    // column, split by vertical separators. Completed objectives drop out so
    // stale drop locations disappear once that step is done.
    let info_objs: Vec<usize> = tooltip_objs
        .iter()
        .copied()
        .filter(|&oi| {
            e.objectives
                .get(oi)
                .is_some_and(|o| o.need > 0 && o.have < o.need && objective_has_spawn_info(o))
        })
        .collect();
    if !info_objs.is_empty() {
        // Paint the horizontal rule and any inter-column dividers manually so
        // they span only the objective block. A bare `ui.separator()` here
        // grows to the tooltip's available width (and vertical ones to its
        // height), stretching the whole tooltip far wider/taller than needed.
        ui.add_space(4.0);
        let divider = ui.visuals().widgets.noninteractive.bg_stroke;
        let rule_y = ui.cursor().top();
        ui.add_space(5.0);
        let block = ui.horizontal_top(|ui| {
            let mut rects: Vec<egui::Rect> = Vec::new();
            for &oi in info_objs.iter() {
                if !rects.is_empty() {
                    ui.add_space(13.0);
                }
                let r = ui
                    .vertical(|ui| {
                        let obj = &e.objectives[oi];
                        let counter = if obj.need > 0 {
                            format!("  {}/{}", obj.have.min(obj.need), obj.need)
                        } else {
                            String::new()
                        };
                        let header = format!("{}{}", obj.label, counter);
                        let w = dungeon_body_width(ui, obj, &header).unwrap_or(240.0);
                        ui.set_max_width(w);
                        render_objective_tip_body(ui, sr, obj, &header);
                    })
                    .response
                    .rect;
                rects.push(r);
            }
            let top = rects.iter().map(|r| r.top()).fold(f32::INFINITY, f32::min);
            let bottom = rects
                .iter()
                .map(|r| r.bottom())
                .fold(f32::NEG_INFINITY, f32::max);
            for pair in rects.windows(2) {
                let x = (pair[0].right() + pair[1].left()) * 0.5;
                ui.painter().vline(x, top..=bottom, divider);
            }
        });
        let rect = block.response.rect;
        ui.painter()
            .hline(rect.left()..=rect.right(), rule_y, divider);
    }
}

/// Resolve an objective's portal id (0 if not a mapped dungeon).
fn obj_portal_id(obj: &ObjectiveView) -> i32 {
    if matches!(obj.kind, ObjectiveKind::Dungeon) {
        obj.dungeon_name
            .as_ref()
            .and_then(|d| get_dungeon_portal_map().get_portal_id(d))
            .unwrap_or(0)
    } else {
        0
    }
}

/// Object id for an objective's tile sprite: a dungeon portal, or a resolved
/// boss sprite for `KillNamed` targets. 0 when neither applies.
fn obj_icon_id(obj: &ObjectiveView) -> i32 {
    let portal = obj_portal_id(obj);
    if portal > 0 {
        portal
    } else {
        obj.boss_id.unwrap_or(0)
    }
}

/// Trophy Hall gravestone sprite, shown for difficulty-range objectives.
const OBJ_GRAVE_ID: i32 = 1282;

/// Resolved artwork for an objective tile. Beyond the classic dungeon
/// portal / boss sprite, non-dungeon objectives now get a meaningful icon
/// such as a gravestone for difficulty ranges, the encounter oryx-head marker,
/// the Oryx's Castle portal for realm closes, and finally the mission's own
/// icon as a catch-all.
#[derive(Clone)]
pub(crate) enum ObjIcon {
    /// A game object sprite (dungeon portal, boss, gravestone), baked outline.
    Object(i32),
    /// A bundled UI icon drawn without an outline (the encounter oryx head).
    Embedded(EmbeddedIcon),
    /// A raw atlas sheet sprite (the mission's own icon), no outline.
    Sheet { sheet: String, idx: i32 },
    /// No resolvable icon; the tile slot is reserved but left blank.
    None,
}

/// The mission's own icon as an [`ObjIcon`], used as the catch-all objective
/// tile: the unique sheet icon when present, else the reward-
/// sprite fallback object.
pub(crate) fn mission_fallback_icon(e: &MissionEntryView) -> ObjIcon {
    if let Some((sheet, idx)) = &e.icon_ref {
        return ObjIcon::Sheet {
            sheet: sheet.clone(),
            idx: *idx,
        };
    }
    match e.icon_object_id {
        Some(id) if id > 0 => ObjIcon::Object(id),
        _ => ObjIcon::None,
    }
}

/// Resolve the tile artwork for one objective, falling back to `mission_icon`
/// for objective types without a dedicated icon.
pub(crate) fn obj_icon(obj: &ObjectiveView, mission_icon: &ObjIcon) -> ObjIcon {
    // 1. Dungeon portal (and timed-dungeon) objectives.
    let portal = obj_portal_id(obj);
    if portal > 0 {
        return ObjIcon::Object(portal);
    }
    // Named-boss kills resolve to the boss sprite.
    if let Some(b) = obj.boss_id {
        if b > 0 {
            return ObjIcon::Object(b);
        }
    }
    // Item 1: X-Y grave-difficulty objectives -> gravestone.
    if matches!(obj.kind, ObjectiveKind::Difficulty) {
        return ObjIcon::Object(OBJ_GRAVE_ID);
    }
    // Item 2: encounter kills -> purple oryx-head marker (no outline).
    if matches!(
        encounter_beacon_group(&obj.target),
        Some(BossGroup::AdeptEncounter | BossGroup::VeteranEncounter)
    ) {
        return ObjIcon::Embedded(EmbeddedIcon::EncounterOryx);
    }
    // Item 3: realm closes -> Oryx's Castle portal.
    if matches!(obj.kind, ObjectiveKind::CloseRealms) {
        let id = portal_sprite_id("Oryx's Castle");
        if id > 0 {
            return ObjIcon::Object(id);
        }
    }
    // Item 4: anything else -> the mission's own icon, scaled to tile size.
    mission_icon.clone()
}

/// Map a mission cond target tag to the Combat-History boss group whose members
/// should be listed in a hover tooltip.
fn encounter_beacon_group(target: &str) -> Option<BossGroup> {
    match target {
        "VETERAN_ENCOUNTER" => Some(BossGroup::VeteranEncounter),
        "ADEPT_ENCOUNTER" => Some(BossGroup::AdeptEncounter),
        "BEACON_GUARDIAN" => Some(BossGroup::BeaconGuardian),
        _ => None,
    }
}

/// A 16px sprite tile for a catalog entry (may fold several sprites) or a single
/// object id, followed by its display name -- one row of a hover tooltip. Large
/// native art is downscaled to fit the 16px cell.
/// The name is slightly dimmed so biome headers stand out above it.
fn tooltip_sprite_row(ui: &mut egui::Ui, sr: &mut SpriteRenderer, sprite_ids: &[i32], name: &str) {
    ui.horizontal(|ui| {
        for &id in sprite_ids {
            tip_icon(ui, sr, normalize_train_sprite(id, name));
        }
        ui.label(RichText::new(name).weak());
    });
}

/// Render a boss-group catalog as fixed-height columns, matching the
/// Combat History filter tooltip so 100+ entries still fit.
fn render_catalog_tooltip(ui: &mut egui::Ui, sr: &mut SpriteRenderer, entries: &[CatalogEntry]) {
    if entries.is_empty() {
        ui.label(RichText::new("No bosses in this group").italics().weak());
        return;
    }
    const COL_ROWS: usize = 18;
    ui.horizontal_top(|ui| {
        for chunk in entries.chunks(COL_ROWS) {
            ui.vertical(|ui| {
                for entry in chunk {
                    tooltip_sprite_row(ui, sr, &entry.sprite_ids, &entry.name);
                }
            });
            ui.add_space(10.0);
        }
    });
}

/// Ordering rank of a biome group in an encounter tooltip: by most-veteran tier
/// among its biomes, then a trailing bucket for encounters with no known biome.
fn encounter_group_rank(biomes: &[String]) -> u8 {
    let drops = get_dungeon_drops();
    biomes
        .iter()
        .filter_map(|b| drops.biome_tier(b))
        .map(|t| match t {
            BiomeTier::Veteran => 0u8,
            BiomeTier::Adept => 1,
            BiomeTier::Seasonal => 2,
            BiomeTier::Rookie => 3,
        })
        .min()
        .unwrap_or(4)
}

/// Render an Adept/Veteran encounter catalog grouped by spawn biome. Membership
/// comes from the authoritative in-game encounter label
/// (`entries`); each entry's biome(s) are looked up from the RealmEye data, so
/// an encounter Deca mislabels is still shown/omitted exactly as the game
/// counts it. Entries with no known biome are listed last under no header.
fn render_encounter_catalog_grouped(
    ui: &mut egui::Ui,
    sr: &mut SpriteRenderer,
    entries: &[CatalogEntry],
) {
    if entries.is_empty() {
        ui.label(RichText::new("No bosses in this group").italics().weak());
        return;
    }
    let drops = get_dungeon_drops();
    // One section per biome; an encounter that spawns in several biomes is
    // repeated under each. Encounters with no known biome are listed last.
    let mut per_biome: HashMap<String, Vec<&CatalogEntry>> = HashMap::new();
    let mut no_biome: Vec<&CatalogEntry> = Vec::new();
    for e in entries {
        let biomes = drops.encounter_biomes(&e.name);
        if biomes.is_empty() {
            no_biome.push(e);
        } else {
            for b in biomes {
                per_biome.entry(b).or_default().push(e);
            }
        }
    }
    let mut ordered: Vec<(String, Vec<&CatalogEntry>)> = per_biome.into_iter().collect();
    ordered.sort_by(|a, b| {
        encounter_group_rank(std::slice::from_ref(&a.0))
            .cmp(&encounter_group_rank(std::slice::from_ref(&b.0)))
            .then_with(|| a.0.cmp(&b.0))
    });

    let mut groups: Vec<TipGroup> = Vec::new();
    for (biome, mut es) in ordered {
        es.sort_by(|a, b| a.name.cmp(&b.name));
        es.dedup_by(|a, b| a.name == b.name);
        let mut rows = vec![TipCell::Biome(biome)];
        for e in es {
            rows.push(TipCell::Bullet {
                sprite_ids: e.sprite_ids.clone(),
                name: e.name.clone(),
                note: None,
            });
        }
        groups.push(TipGroup {
            separated: true,
            rows,
        });
    }
    if !no_biome.is_empty() {
        no_biome.sort_by(|a, b| a.name.cmp(&b.name));
        no_biome.dedup_by(|a, b| a.name == b.name);
        let rows = no_biome
            .into_iter()
            .map(|e| TipCell::Bullet {
                sprite_ids: e.sprite_ids.clone(),
                name: e.name.clone(),
                note: None,
            })
            .collect();
        groups.push(TipGroup {
            separated: true,
            rows,
        });
    }
    flow_column_groups(ui, sr, groups);
}

/// Beacon sprite object id for a RealmEye biome name -- the "Actual Active
/// Beacon" series shown in the character card's BIOME ENEMY KILLS section
/// series. 0 = no beacon object (e.g. Relentless Springs).
fn biome_beacon_sprite(biome: &str) -> i32 {
    match biome {
        "Abandoned City" => 9845,
        "Beach" => 9840,
        // The synthetic "Any Biome" bucket (only Spec Pen's Spectral Jailer)
        // reuses the Carboniferous beacon, drawn as a black silhouette so it
        // reads as a generic Veteran beacon rather than a specific one.
        "Any Biome" | "Carboniferous" => 9852,
        "Coral Reefs" => 9846,
        "Dead Church" => 9847,
        "Deep Sea Abyss" => 9853,
        "Eternal Frost" => 65278,
        "Floral Escape" => 9854,
        "Haunted Hallows" => 9848,
        "High Desert" | "Mid Desert" => 9843,
        "High Forest" | "Low Forest" | "Nature Ruins" => 9841,
        "High Plains" | "Mid Plains" => 9844,
        "Risen Hell" => 9849,
        // "Spring Big 8" (id 10507): the Relentless Springs decorative egg
        // beacon, which randomly draws one of its 8 egg frames in-game. No
        // dedicated "Active Beacon" object ships for this seasonal biome.
        "Relentless Springs" => 10507,
        "Runic Tundra" => 9856,
        "Sanguine Forest" => 9855,
        "Shipwreck Cove" => 9850,
        "Sprite Forest" => 9851,
        "Undead Forest" => 9842,
        _ => 0,
    }
}

/// Beacon sprite for a Legion Beacon Guardian, drawn to the LEFT of the guardian
/// in the Beacon Guardian tooltip.
fn guardian_beacon_sprite(name: &str) -> i32 {
    match name {
        "Legion City Guardian" => 9845,
        "Legion Colonel" => 9852,
        "Legion Church Guardian" => 9847,
        "Legion Commissioner" => 9854,
        "Legion Cove Guardian" => 9850,
        "Legion Desert Guard" => 9843,
        "Legion Fey Guardian" => 9851,
        "Legion Graveyard Guardian" => 9848,
        "Legion Hell Guardian" => 9849,
        "Legion Missionary" => 9855,
        "Legion Nature Ruins Guard" | "Legion Forest Guard" => 9841,
        "Legion Plains Guard" => 9844,
        "Legion Principal" => 9856,
        "Legion Undead Forest Guard" => 9842,
        _ => 0,
    }
}

/// Header label for a biome. Seasonal event biomes get an explanatory prefix
/// for seasonal event biomes; events are Oryxmas (Eternal Frost) then Easter
/// (Relentless Springs). The synthetic "Any Biome" bucket reads naturally.
fn biome_header_text(biome: &str) -> String {
    match biome {
        "Any Biome" => "Any Veteran biome".to_string(),
        _ => match biome_event_suffix(biome) {
            Some(suffix) => format!("{biome} {suffix}"),
            None => biome.to_string(),
        },
    }
}

/// The seasonal-event context suffix for a biome header, when it only appears on
/// the map during a limited-time event. Rendered dimmed/small next to the name.
fn biome_event_suffix(biome: &str) -> Option<&'static str> {
    match biome {
        "Eternal Frost" => Some("(during Oryxmas event)"),
        "Relentless Springs" => Some("(during Easter event)"),
        _ => None,
    }
}

/// Resolve an enemy display name to a sprite object id (0 when unknown). A few
/// enemies whose RealmEye display name differs from the in-game id_name are
/// resolved via that id_name.
fn enemy_sprite_id(name: &str) -> i32 {
    let am = get_asset_manager();
    // A few display names collide with an older/decorative object that shares the
    // name (e.g. "Candy Gnome" resolves to a Candyland setpiece by display name);
    // resolve those to the correct enemy via its internal id_name first.
    let override_id_name = match name {
        "Candy Gnome" => Some("New Candy Gnome"),
        // The enemy Suit of Armor (0x0D71) carries an empty display name, so the
        // display index instead maps "Suit of Armor" to a decorative GameObject
        // sharing the name; resolve the enemy directly via its id_name.
        "Suit of Armor" => Some("Suit of Armor"),
        _ => None,
    };
    if let Some(idn) = override_id_name {
        if let Some(id) = am.object_id_for_name(idn) {
            return id;
        }
    }
    if let Some(id) = am.object_id_for_display_name(name) {
        return id;
    }
    let id_name = match name {
        "Spectral Jailer" => Some("SpecPen Spectral Jailer"),
        _ => None,
    };
    if let Some(idn) = id_name {
        if let Some(id) = am.object_id_for_name(idn) {
            return id;
        }
    }
    // Some enemies (e.g. Alluring Blossom) carry an empty display field in the
    // object list and are only indexed by their internal id_name, so they never
    // reach the display-name index. Fall back to an id_name match before giving
    // up, which keeps them from being silently dropped from drop tooltips.
    if let Some(id) = am.object_id_for_name(name) {
        return id;
    }
    0
}

/// The catalog for an encounter/beacon group, with known exceptions applied:
/// Mysterious Crystal is excluded from the Adept encounter list (its
/// boss is an Adept Hero of Oryx). Beacon Guardian entries additionally get
/// their beacon sprite prepended.
fn catalog_for_group(group: BossGroup) -> Vec<CatalogEntry> {
    let mut list = group.catalog_bosses();
    match group {
        BossGroup::AdeptEncounter => list.retain(|e| e.name != "Mysterious Crystal"),
        BossGroup::BeaconGuardian => {
            for e in &mut list {
                let beacon = guardian_beacon_sprite(&e.name);
                if beacon > 0 {
                    e.sprite_ids.insert(0, beacon);
                }
            }
        }
        _ => {}
    }
    list
}

/// A biome section header: beacon sprite (when the biome has one) + strong
/// biome name (with the seasonal-event prefix where relevant).
fn biome_header_row(ui: &mut egui::Ui, sr: &mut SpriteRenderer, biome: &str) {
    let beacon_id = biome_beacon_sprite(biome);
    ui.horizontal(|ui| {
        // "Any Biome" draws its beacon as a solid black silhouette (shape only,
        // no specific-beacon detail) since it stands for "any Veteran biome".
        if biome == "Any Biome" {
            tip_icon_tinted(ui, sr, beacon_id, Color32::BLACK);
            ui.label(RichText::new(biome_header_text(biome)).strong());
        } else {
            tip_icon(ui, sr, beacon_id);
            ui.label(RichText::new(biome).strong());
            if let Some(suffix) = biome_event_suffix(biome) {
                ui.label(RichText::new(suffix).small().color(Color32::GRAY));
            }
        }
    });
}

/// Colour for the spawn-cap annotations ("(respawnable)"/"(limited)") shown on
/// exaltation-dungeon drop bullets, so they read as a distinct annotation rather
/// than part of the enemy name.
const SPAWN_NOTE_COLOR: Color32 = Color32::from_rgb(120, 185, 235);

/// A single entry row under a biome: bullet + sprite(s) + dimmed name.
fn bullet_row(ui: &mut egui::Ui, sr: &mut SpriteRenderer, sprite_ids: &[i32], name: &str) {
    bullet_row_noted(ui, sr, sprite_ids, name, None);
}

/// [`bullet_row`] with an optional trailing spawn-cap label (e.g. "respawnable"
/// / "limited"), drawn dimmer than the name so it reads as an annotation. The
/// label is kept separate from `name` so the sprite still resolves against the
/// raw enemy name (see [`normalize_train_sprite`]).
fn bullet_row_noted(
    ui: &mut egui::Ui,
    sr: &mut SpriteRenderer,
    sprite_ids: &[i32],
    name: &str,
    note: Option<&str>,
) {
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(RichText::new("\u{2022}").weak());
        for &id in sprite_ids {
            tip_icon(ui, sr, normalize_train_sprite(id, name));
        }
        ui.label(RichText::new(name).weak());
        if let Some(note) = note {
            ui.label(
                RichText::new(format!("({note})"))
                    .color(SPAWN_NOTE_COLOR)
                    .small(),
            );
        }
    });
}

/// A curated "parent (child)" bullet: [icon]parent ([icon]child), with the child
/// icon nested inside the parentheses so both the quest-facing NPC (parent) and
/// the actual portal/loot dropper (child) are shown. `note` appends a spawn-cap
/// label after the closing paren.
fn parent_bullet_row(
    ui: &mut egui::Ui,
    sr: &mut SpriteRenderer,
    parent_id: i32,
    parent: &str,
    child_id: i32,
    child: &str,
    note: Option<&str>,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        ui.add_space(6.0);
        ui.label(RichText::new("\u{2022}").weak());
        tip_icon(ui, sr, parent_id);
        ui.label(RichText::new(format!("{parent} (")).weak());
        tip_icon(ui, sr, child_id);
        ui.label(RichText::new(format!("{child})")).weak());
        if let Some(note) = note {
            ui.label(
                RichText::new(format!("({note})"))
                    .color(SPAWN_NOTE_COLOR)
                    .small(),
            );
        }
    });
}

/// Render the per-biome drop model: one section per biome, ordered
/// Veteran -> Adept -> Seasonal -> Rookie, each with a beacon-headed biome name
/// followed by its bulleted enemies (Encounters -> Heroes of Oryx -> regulars).
/// An enemy that spans several biomes is repeated under each. Biomes are
/// separated by a horizontal rule. Enemies with no resolvable sprite (event
/// reskins share a base object id and have none) are hidden, and a biome left
/// with no enemies is skipped entirely.
/// One row of a column-flowed tooltip body: either a strong biome/section
/// header or a bulleted entry (bullet + sprite(s) + dimmed name).
enum TipCell {
    /// A biome header row (rendered via [`biome_header_row`]).
    Biome(String),
    /// A bulleted entry row (rendered via [`bullet_row_noted`]). `note` is an
    /// optional trailing spawn-cap label (e.g. "respawnable"/"limited").
    Bullet {
        sprite_ids: Vec<i32>,
        name: String,
        note: Option<String>,
    },
    /// A curated "parent (child)" encounter row: the quest/map points at the
    /// `parent` NPC (which carries the per-realm spawn cap) while the portal and
    /// loot drop from the `child`. Rendered as [icon]parent ([icon]child) with
    /// an optional trailing cap label.
    ParentBullet {
        parent_id: i32,
        parent: String,
        child_id: i32,
        child: String,
        note: Option<String>,
    },
}

/// An unbreakable group of rows kept together within a single column (e.g. a
/// biome header plus its enemies). `separated` prefixes a horizontal rule when
/// the group is not the first in its column.
struct TipGroup {
    separated: bool,
    rows: Vec<TipCell>,
}

fn render_tip_cell(ui: &mut egui::Ui, sr: &mut SpriteRenderer, cell: &TipCell) {
    match cell {
        TipCell::Biome(b) => biome_header_row(ui, sr, b),
        TipCell::Bullet {
            sprite_ids,
            name,
            note,
        } => bullet_row_noted(ui, sr, sprite_ids, name, note.as_deref()),
        TipCell::ParentBullet {
            parent_id,
            parent,
            child_id,
            child,
            note,
        } => parent_bullet_row(
            ui,
            sr,
            *parent_id,
            parent,
            *child_id,
            child,
            note.as_deref(),
        ),
    }
}

fn render_tip_column(ui: &mut egui::Ui, sr: &mut SpriteRenderer, col: &[TipGroup]) {
    let mut first = true;
    for g in col {
        if !first && g.separated {
            ui.separator();
        }
        first = false;
        for cell in &g.rows {
            render_tip_cell(ui, sr, cell);
        }
    }
}

/// Lay a sequence of row-groups out in vertical columns, starting a new column
/// whenever the current one would grow past the app height. Keeps tall mission
/// tooltips (long dungeon/enemy lists) inside the window on short screens
/// (e.g. 1366x768) by flowing into additional, wider columns instead of
/// overflowing vertically and being clipped.
fn flow_column_groups(ui: &mut egui::Ui, sr: &mut SpriteRenderer, groups: Vec<TipGroup>) {
    if groups.is_empty() {
        return;
    }
    // Approximate per-row height (icon + row spacing) and the usable height.
    // Budget the column height from the total screen height minus a fixed
    // reserve (tooltip header + top/bottom margins). Deriving it from the
    // tooltip's on-screen position (`cursor().top()`) made `max_rows` depend on
    // where egui placed the tooltip -- but that placement depends on the
    // tooltip's height, which depends on the column count, which depends on
    // `max_rows`. Near a screen edge that feedback loop made the layout
    // oscillate between column counts every frame.
    let row_h = TIP_ICON + 4.0;
    let screen = ui.ctx().content_rect();
    // Reserve room for the mission header above the list (badge, name, desc,
    // requirements, progress bar, reward row, and the objective label) plus
    // top/bottom margins. The tall "Dungeons that count" difficulty tooltips
    // clip when this is underestimated, so keep it generous.
    let avail = (screen.height() - 300.0).max(180.0);
    let max_rows = ((avail / row_h).floor() as usize).max(6);

    // Balance the groups across the minimum number of columns that fit the
    // height budget, rather than greedily filling each column to `max_rows`
    // (which left a full first column beside a nearly-empty second one). A
    // group taller than the target still occupies its own column.
    let total_rows: usize = groups.iter().map(|g| g.rows.len()).sum();
    let needed_cols = total_rows.div_ceil(max_rows).max(1);
    let target = total_rows.div_ceil(needed_cols).max(1);

    let mut columns: Vec<Vec<TipGroup>> = Vec::new();
    let mut cur: Vec<TipGroup> = Vec::new();
    let mut cur_rows = 0usize;
    for g in groups {
        let gr = g.rows.len();
        if !cur.is_empty() && cur_rows + gr > target {
            columns.push(std::mem::take(&mut cur));
            cur_rows = 0;
        }
        cur_rows += gr;
        cur.push(g);
    }
    if !cur.is_empty() {
        columns.push(cur);
    }

    if columns.len() <= 1 {
        render_tip_column(ui, sr, &columns[0]);
        return;
    }
    // Size each column to its own longest row so the in-column separators span
    // only that column (not the whole tooltip) and the next column sits right
    // beside it instead of being pushed far right.
    let widths: Vec<f32> = columns.iter().map(|c| tip_column_width(ui, c)).collect();
    let gap = 10.0;
    let total: f32 = widths.iter().sum::<f32>() + gap * (columns.len() as f32 - 1.0) + 8.0;
    ui.set_max_width(total);
    ui.horizontal_top(|ui| {
        for (ci, col) in columns.iter().enumerate() {
            if ci > 0 {
                ui.add_space(gap);
            }
            let w = widths[ci];
            ui.allocate_ui_with_layout(
                egui::vec2(w, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_max_width(w);
                    render_tip_column(ui, sr, col);
                },
            );
        }
    });
}

/// Estimate a tooltip column's natural width from its longest row (biome header
/// or bulleted entry), so the column can be sized to fit its text exactly and
/// its separators don't stretch across the whole tooltip.
fn tip_column_width(ui: &egui::Ui, col: &[TipGroup]) -> f32 {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let sx = ui.spacing().item_spacing.x;
    let measure = |text: &str| -> f32 {
        ui.fonts_mut(|f| {
            f.layout_no_wrap(text.to_string(), font.clone(), egui::Color32::WHITE)
                .size()
                .x
        })
    };
    let bullet_w = measure("\u{2022}");
    let mut w = 0.0f32;
    for g in col {
        for cell in &g.rows {
            let cw = match cell {
                TipCell::Biome(b) => TIP_ICON + sx + measure(&biome_header_text(b)),
                TipCell::Bullet {
                    sprite_ids,
                    name,
                    note,
                } => {
                    let mut cw = 6.0
                        + bullet_w
                        + sx
                        + sprite_ids.len() as f32 * (TIP_ICON + sx)
                        + measure(name);
                    if let Some(n) = note {
                        cw += sx + measure(&format!("({n})"));
                    }
                    cw
                }
                TipCell::ParentBullet {
                    parent,
                    child,
                    note,
                    ..
                } => {
                    let mut cw = 6.0
                        + bullet_w
                        + sx
                        + (TIP_ICON + sx)
                        + measure(&format!("{parent} ("))
                        + sx
                        + (TIP_ICON + sx)
                        + measure(&format!("{child})"));
                    if let Some(n) = note {
                        cw += sx + measure(&format!("({n})"));
                    }
                    cw
                }
            };
            if cw > w {
                w = cw;
            }
        }
    }
    w.ceil() + 4.0
}

/// Natural width for a dungeon-drop objective column, so the intro sentence
/// wraps to (and the biome separators stop at) the widest biome/monster row
/// instead of a fixed 240px. Returns `None` only for non-dungeon objectives,
/// which keep the default width.
fn dungeon_body_width(ui: &egui::Ui, obj: &ObjectiveView, header: &str) -> Option<f32> {
    if !matches!(obj.kind, ObjectiveKind::Dungeon) {
        return None;
    }
    let name = obj.dungeon_name.as_ref()?;
    let low = name.trim().to_ascii_lowercase();
    let font = egui::TextStyle::Body.resolve(ui.style());
    let sx = ui.spacing().item_spacing.x;
    let measure = |text: &str| -> f32 {
        ui.fonts_mut(|f| {
            f.layout_no_wrap(text.to_string(), font.clone(), egui::Color32::WHITE)
                .size()
                .x
        })
    };
    // Wrappable text only needs its widest single word to fit; the atomic
    // [icon]+name unit needs the icon plus the whole name.
    let longest_word = |s: &str| s.split_whitespace().map(&measure).fold(0.0f32, f32::max);
    let bullet_w = measure("\u{2022}");
    // The header row (portal icon + dungeon name) is a single non-wrapping line.
    let mut w = TIP_ICON + sx + measure(header);
    // The bespoke Void / Oryx's Sanctuary layouts render their own access
    // sentence + drop rows; snap to the widest atomic access token and drop row.
    if low == "void" || low == "the void" {
        w = w.max(access_tokens_width(ui, &void_access_tokens()));
        let sections = get_dungeon_drops().tooltip_model("Lost Halls");
        w = w.max(tip_column_width(ui, &dungeon_section_groups(&sections)));
        return Some(w.ceil().clamp(150.0, 240.0));
    }
    if low == "oryx's sanctuary" {
        w = w.max(access_tokens_width(ui, &oryx_sanctuary_access_tokens()));
        w = w.max(longest_word(
            "Requires activating special items inside Oryx's Chamber and Wine Cellar after boss' deaths:",
        ));
        for item in [
            "Wine Cellar Incantation",
            "Helmet Rune",
            "Shield Rune",
            "Sword Rune",
        ] {
            w = w.max(6.0 + bullet_w + sx + (TIP_ICON + sx) + measure(item));
        }
        return Some(w.ceil().clamp(150.0, 240.0));
    }
    let parts = build_dungeon_parts(name)?;
    for part in &parts {
        let cw = match part {
            TipPart::Text(s) => longest_word(s),
            TipPart::Portal {
                prefix,
                name,
                suffix,
                ..
            } => (TIP_ICON + 3.0 + measure(name))
                .max(longest_word(prefix))
                .max(longest_word(suffix)),
            TipPart::Sections(secs) => tip_column_width(ui, &dungeon_section_groups(secs)),
            TipPart::Enemies(names) => names
                .iter()
                .map(|n| 6.0 + bullet_w + sx + (TIP_ICON + sx) + measure(n))
                .fold(0.0f32, f32::max),
        };
        if cw > w {
            w = cw;
        }
    }
    Some(w.ceil().clamp(150.0, 240.0))
}
/// reskins share a base object id and have none) are hidden, and a biome left
/// with no enemies is skipped entirely. Tall lists flow into
/// multiple columns so they stay within the window height.
fn render_dungeon_sections(ui: &mut egui::Ui, sr: &mut SpriteRenderer, sections: &[BiomeSection]) {
    flow_column_groups(ui, sr, dungeon_section_groups(sections));
}

/// A resolved drop-model row before its spawn-cap label is decided.
enum RowKind {
    /// A plain bullet: sprite + enemy name.
    Simple { id: i32, name: String },
    /// A curated "parent (child)" bullet (see [`TipCell::ParentBullet`]).
    Parent {
        parent_id: i32,
        parent: String,
        child_id: i32,
        child: String,
    },
}

/// Resolve an encounter/enemy `name` to its render pieces, whether it is a
/// per-realm spawn-`limited` encounter, and whether it should carry a spawn-cap
/// label at all (`labelable`). Returns `None` when it has no resolvable sprite
/// (event reskins share a base object id and have none).
///
/// Only Encounter-category sources (Veteran/Adept encounters and their seasonal
/// reskins) are labelled; Heroes of Oryx and regular biome enemies are always
/// respawnable and left unlabelled. The Spectral Jailer is an explicit exception
/// -- a rare regular enemy with no quest marker, so it keeps a "(respawnable)"
/// tag to signal it can still be farmed.
fn resolve_enemy_row(name: &str) -> Option<(RowKind, bool, bool)> {
    // Crystal Worm Father's per-realm cap lives on its Dwarf Miner parent NPC
    // (the quest/map target); the portal and loot drop from the Worm Father, so
    // both are shown as [icon]Dwarf Miner ([icon]Crystal Worm Father).
    if name.eq_ignore_ascii_case("Crystal Worm Father") {
        let child_id = enemy_sprite_id(name);
        if child_id == 0 {
            return None;
        }
        return Some((
            RowKind::Parent {
                parent_id: enemy_sprite_id("Dwarf Miner"),
                parent: "Dwarf Miner".to_string(),
                child_id,
                child: name.to_string(),
            },
            true,
            true,
        ));
    }
    let id = enemy_sprite_id(name);
    if id == 0 {
        return None;
    }
    let labelable =
        get_dungeon_drops().is_encounter(name) || name.eq_ignore_ascii_case("Spectral Jailer");
    Some((
        RowKind::Simple {
            id,
            name: name.to_string(),
        },
        encounter_spawn_limited(name),
        labelable,
    ))
}

/// Turn a [`RowKind`] into a renderable cell, attaching a spawn-cap `note`
/// ("respawnable"/"limited") when `note` should be shown.
fn row_cell(kind: RowKind, limited: bool, show_note: bool) -> TipCell {
    let note = show_note.then(|| if limited { "limited" } else { "respawnable" }.to_string());
    match kind {
        RowKind::Simple { id, name } => TipCell::Bullet {
            sprite_ids: vec![id],
            name,
            note,
        },
        RowKind::Parent {
            parent_id,
            parent,
            child_id,
            child,
        } => TipCell::ParentBullet {
            parent_id,
            parent,
            child_id,
            child,
            note,
        },
    }
}

/// Category ordering rank for a drop source: the tooltip lists the most
/// readily-available sources first -- common biome enemies, then Heroes of Oryx,
/// then Encounters (which only have a chance to spawn).
fn drop_category_rank(cat: DropCategory) -> u8 {
    match cat {
        DropCategory::Regular => 0,
        DropCategory::Hero => 1,
        DropCategory::Encounter => 2,
        DropCategory::Beacon => 3,
    }
}

/// Realm-tier ordering rank for a biome section: tooltips list biomes by tier,
/// Veteran first, with seasonal biomes (only on the map a couple months a year)
/// last so they always sink to the bottom.
fn biome_tier_rank(tier: BiomeTier) -> u8 {
    match tier {
        BiomeTier::Veteran => 0,
        BiomeTier::Adept => 1,
        BiomeTier::Rookie => 2,
        BiomeTier::Seasonal => 3,
    }
}

/// Build the per-biome row groups for a dungeon's drop model (see
/// [`render_dungeon_sections`]), so the same grouping can be measured (to size
/// the objective column) and rendered.
///
/// The basic structure is always preserved: common biome enemies first, then
/// Heroes of Oryx, then Encounters -- reflecting how readily each can be farmed.
/// Encounter sources (Veteran/Adept encounters and their reskins) and the
/// Spectral Jailer are annotated ("respawnable"/"limited"); Heroes of Oryx and
/// regular biome enemies are always respawnable and left unlabelled. Within a
/// category, respawnable encounters come before limited ones. Biomes are ordered
/// by realm tier (Veteran -> Adept -> Rookie -> Seasonal), then by number of drop
/// sources (descending) so the biome most likely to yield the target leads.
/// Seasonal biomes (Eternal Frost, Relentless Springs) are only on the map a
/// couple months a year, so their tier ranks last and they always sink to the
/// bottom. All sorts are stable, preserving the existing name order within each
/// rank.
fn dungeon_section_groups(sections: &[BiomeSection]) -> Vec<TipGroup> {
    // Row: (kind, limited, labelable, category_rank).
    let mut built: Vec<(String, u8, Vec<(RowKind, bool, bool, u8)>)> = Vec::new();
    for section in sections {
        let rows: Vec<(RowKind, bool, bool, u8)> = section
            .enemies
            .iter()
            .zip(section.categories.iter())
            .filter_map(|(n, cat)| {
                resolve_enemy_row(n).map(|(k, l, la)| (k, l, la, drop_category_rank(*cat)))
            })
            .collect();
        if rows.is_empty() {
            continue;
        }
        built.push((section.biome.clone(), biome_tier_rank(section.tier), rows));
    }
    // Within each biome, order by category (common -> hero -> encounter), then
    // respawnable-before-limited among encounters. Stable, so tier/name order is
    // preserved within a rank.
    for (_, _, rows) in &mut built {
        rows.sort_by(|a, b| a.3.cmp(&b.3).then_with(|| a.1.cmp(&b.1)));
    }
    // Between biomes: order by realm tier (Veteran -> Adept -> Rookie ->
    // Seasonal), then by number of drop sources (descending). Stable, preserving
    // the incoming name order within a rank.
    built.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| b.2.len().cmp(&a.2.len())));
    built
        .into_iter()
        .map(|(biome, _, rows)| {
            let mut cells = vec![TipCell::Biome(biome)];
            cells.extend(
                rows.into_iter()
                    .map(|(kind, limited, labelable, _)| row_cell(kind, limited, labelable)),
            );
            TipGroup {
                separated: true,
                rows: cells,
            }
        })
        .collect()
}

/// One renderable part of a dungeon-drop tooltip.
enum TipPart {
    /// A plain line of text.
    Text(String),
    /// A parent-portal reference line: `prefix` text, the portal sprite, then
    /// `name`+`suffix` text (used by the special "found inside X" dungeons).
    Portal {
        prefix: String,
        id: i32,
        name: String,
        suffix: String,
    },
    /// A biome-grouped drop list.
    Sections(Vec<BiomeSection>),
    /// A bare list of enemy rows (no biome headers).
    Enemies(Vec<String>),
}

fn portal_sprite_id(name: &str) -> i32 {
    get_dungeon_portal_map().get_portal_id(name).unwrap_or(0)
}

/// Special dungeons that are not found directly in the realm: Cultist Hideout
/// and Void live inside Lost Halls; Crystal Cavern inside Fungal Cavern and from
/// a Relentless Springs (Easter) enemy. Returns the custom tooltip parts, or
/// `None` for ordinary dungeons.
fn special_dungeon_parts(name: &str) -> Option<Vec<TipPart>> {
    let drops = get_dungeon_drops();
    let n = name.trim().to_ascii_lowercase();
    match n.as_str() {
        "cultist hideout" => {
            let sections = drops.tooltip_model("Lost Halls");
            if sections.is_empty() {
                return None;
            }
            Some(vec![
                TipPart::Portal {
                    prefix: "Can be found inside ".to_string(),
                    id: portal_sprite_id("Lost Halls"),
                    name: "Lost Halls".to_string(),
                    suffix: " which drops from:".to_string(),
                },
                TipPart::Sections(sections),
            ])
        }
        "crystal cavern" => {
            let fungal = drops.tooltip_model("Fungal Cavern");
            // Crystal Cavern's own realm drop source (the Relentless Springs
            // Easter enemy).
            let easter_enemies: Vec<String> = drops
                .tooltip_model("Crystal Cavern")
                .into_iter()
                .flat_map(|s| s.enemies)
                .collect();
            let mut parts = vec![TipPart::Text("Can be found:".to_string())];
            if !fungal.is_empty() {
                parts.push(TipPart::Portal {
                    prefix: "1) Inside ".to_string(),
                    id: portal_sprite_id("Fungal Cavern"),
                    name: "Fungal Cavern".to_string(),
                    suffix: " which drops from:".to_string(),
                });
                parts.push(TipPart::Sections(fungal));
            }
            if !easter_enemies.is_empty() {
                parts.push(TipPart::Text(
                    "2) During Easter event in Relentless Springs, from:".to_string(),
                ));
                parts.push(TipPart::Enemies(easter_enemies));
            }
            Some(parts)
        }
        _ => None,
    }
}

/// Build the ordered tooltip parts for a dungeon objective, or `None` when the
/// dungeon has no known sources.
fn build_dungeon_parts(name: &str) -> Option<Vec<TipPart>> {
    if let Some(parts) = special_dungeon_parts(name) {
        return Some(parts);
    }
    let sections = get_dungeon_drops().tooltip_model(name);
    if sections.is_empty() {
        return None;
    }
    Some(vec![
        TipPart::Text("Drops from:".to_string()),
        TipPart::Sections(sections),
    ])
}

/// True when [`render_dungeon_tip_body`] would render real drop/access detail
/// for `name` (not just a bare header). Used to decide whether to append a
/// dungeon column to the Taskbar quest tooltip.
pub(crate) fn dungeon_has_tip_info(name: &str) -> bool {
    let low = name.trim().to_ascii_lowercase();
    if low == "oryx's sanctuary"
        || low == "void"
        || low == "the void"
        || low == "oryx's castle"
        || low == "oryx's chamber"
        || low == "wine cellar"
    {
        return true;
    }
    if alien_dungeon_access(&low).is_some() {
        return true;
    }
    build_dungeon_parts(name).is_some()
}

/// Access info for the alien wormhole dungeons, keyed by lowercased dungeon
/// name: `(biome_tier, encounter_name, encounter_biome)`. The base dungeons
/// (Malogia/Untaris/Katalund/Forax) spawn in Adept biomes after any Alien UFO is
/// defeated; the Neo variants spawn in Veteran biomes after Commander Calbrik in
/// Runic Tundra. Returns `None` for any other dungeon.
fn alien_dungeon_access(low: &str) -> Option<(&'static str, &'static str, Option<&'static str>)> {
    match low {
        "malogia" | "untaris" | "katalund" | "forax" => Some(("Adept", "Alien UFO", None)),
        "neo malogia" | "neo untaris" | "neo katalund" | "neo forax" => {
            Some(("Veteran", "Commander Calbrik", Some("Runic Tundra")))
        }
        _ => None,
    }
}

/// A single non-breaking token in a wrappable access sentence: either a plain
/// word or an [icon]+name unit that must stay glued together on one line.
enum AccessTok {
    Word(String),
    Icon(i32, String),
}

/// Split `s` into individual word tokens (each wraps on its own).
fn access_words(list: &mut Vec<AccessTok>, s: &str) {
    for w in s.split_whitespace() {
        list.push(AccessTok::Word(w.to_string()));
    }
}

/// Width budget of the widest single atomic token (a word, or an [icon]+name
/// unit). The snapped column must be at least this wide so no token overflows.
fn access_tokens_width(ui: &egui::Ui, toks: &[AccessTok]) -> f32 {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let measure = |t: &str| -> f32 {
        ui.fonts_mut(|f| {
            f.layout_no_wrap(t.to_string(), font.clone(), egui::Color32::WHITE)
                .size()
                .x
        })
    };
    toks.iter()
        .map(|t| match t {
            AccessTok::Word(w) => measure(w),
            AccessTok::Icon(_, name) => TIP_ICON + 3.0 + measure(name),
        })
        .fold(0.0f32, f32::max)
}

/// Render a wrappable access sentence, greedily breaking lines to the column's
/// available width. Words wrap individually; an [icon]+name unit stays glued and
/// moves to the next line as a whole when it doesn't fit -- so the line break
/// always lands before the whole unit, never between the icon and its name, nor
/// between the words of the name.
fn render_access_tokens(ui: &mut egui::Ui, sr: &mut SpriteRenderer, toks: &[AccessTok]) {
    const SX: f32 = 3.0;
    let avail = ui.available_width();
    // Measure every token first (immutable borrow), then render (mutable borrow).
    let widths: Vec<f32> = {
        let font = egui::TextStyle::Body.resolve(ui.style());
        let measure = |t: &str| -> f32 {
            ui.fonts_mut(|f| {
                f.layout_no_wrap(t.to_string(), font.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            })
        };
        toks.iter()
            .map(|t| match t {
                AccessTok::Word(w) => measure(w),
                AccessTok::Icon(_, name) => TIP_ICON + 3.0 + measure(name),
            })
            .collect()
    };
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut x = 0.0f32;
    for (i, w) in widths.iter().enumerate() {
        if cur.is_empty() {
            cur.push(i);
            x = *w;
        } else if x + SX + w <= avail {
            cur.push(i);
            x += SX + w;
        } else {
            rows.push(std::mem::take(&mut cur));
            cur.push(i);
            x = *w;
        }
    }
    if !cur.is_empty() {
        rows.push(cur);
    }
    for row in rows {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = SX;
            for &i in &row {
                match &toks[i] {
                    AccessTok::Word(w) => {
                        ui.label(RichText::new(w).strong());
                    }
                    AccessTok::Icon(id, name) => icon_word(ui, sr, *id, name),
                }
            }
        });
    }
}

/// The wrappable access sentence for the Void (opened from within Lost Halls).
fn void_access_tokens() -> Vec<AccessTok> {
    let mut t = Vec::new();
    access_words(&mut t, "Can be accessed by opening");
    t.push(AccessTok::Icon(
        item_sprite_id("Vial of Pure Darkness"),
        "Vial Of Pure Darkness".to_string(),
    ));
    access_words(&mut t, "after defeating");
    t.push(AccessTok::Icon(
        enemy_sprite_id("Marble Colossus"),
        "Marble Colossus".to_string(),
    ));
    access_words(&mut t, "in");
    t.push(AccessTok::Icon(
        portal_sprite_id("Lost Halls"),
        "Lost Halls".to_string(),
    ));
    access_words(&mut t, "which drops from:");
    t
}

/// The wrappable access sentence for Oryx's Sanctuary.
fn oryx_sanctuary_access_tokens() -> Vec<AccessTok> {
    let mut t = Vec::new();
    access_words(&mut t, "Can be accessed by progressing through");
    t.push(AccessTok::Icon(
        portal_sprite_id("Oryx's Castle"),
        "Oryx's Castle,".to_string(),
    ));
    t.push(AccessTok::Icon(
        portal_sprite_id("Oryx's Chamber"),
        "Oryx's Chamber".to_string(),
    ));
    access_words(&mut t, "and");
    t.push(AccessTok::Icon(
        portal_sprite_id("Wine Cellar"),
        "Wine Cellar".to_string(),
    ));
    access_words(&mut t, "subsequently.");
    t
}

fn render_tip_parts(ui: &mut egui::Ui, sr: &mut SpriteRenderer, parts: &[TipPart]) {
    for part in parts {
        match part {
            TipPart::Text(s) => {
                ui.label(RichText::new(s).strong());
            }
            TipPart::Portal {
                prefix,
                id,
                name,
                suffix,
            } => {
                // Wrap the sentence to the tightened column, keeping the portal
                // sprite glued to its name so a line break lands before the whole
                // [icon]+name unit, never between the icon and the dungeon name.
                let mut toks = Vec::new();
                access_words(&mut toks, prefix);
                toks.push(AccessTok::Icon(*id, name.clone()));
                access_words(&mut toks, suffix);
                render_access_tokens(ui, sr, &toks);
            }
            TipPart::Sections(secs) => render_dungeon_sections(ui, sr, secs),
            TipPart::Enemies(names) => {
                for n in names {
                    let id = enemy_sprite_id(n);
                    if id == 0 {
                        continue;
                    }
                    bullet_row(ui, sr, &[id], n);
                }
            }
        }
    }
}

/// Object id for an item/consumable (runes, the Vial of Pure Darkness, the Wine
/// Cellar Incantation). These carry their name in the `id_name` field with an
/// empty display name, so fall back to id-name lookup. 0 when unknown.
fn item_sprite_id(name: &str) -> i32 {
    let am = get_asset_manager();
    am.object_id_for_display_name(name)
        .or_else(|| am.object_id_for_name(name))
        .unwrap_or(0)
}

/// A pixel-snapped, crisp sprite drawn inline within a tooltip row -- no bullet
/// or name. Sizes the box to the sprite's pixel-perfect render size at
/// `TIP_ICON` and draws with `draw_sprite_in_rect` (integer upscaling for small
/// art, fit-to-box for large art), matching the crisp inline icons used in other
/// tooltips such as the Exaltations "Exalted in dungeons" tip.
fn tip_icon(ui: &mut egui::Ui, sr: &mut SpriteRenderer, id: i32) {
    let size = if id > 0 {
        let ppp = ui.ctx().pixels_per_point();
        sr.rendered_sprite_size(id, TIP_ICON, ppp)
    } else {
        egui::vec2(TIP_ICON, TIP_ICON)
    };
    let (r, _) = ui.allocate_exact_size(size, Sense::hover());
    if id > 0 {
        sr.draw_sprite_in_rect(ui, id, r);
    }
}

/// Like [`tip_icon`] but multiplies the sprite by `tint`. A solid `BLACK` tint
/// (RGB 0, alpha preserved) renders the sprite as a shape-only silhouette.
fn tip_icon_tinted(ui: &mut egui::Ui, sr: &mut SpriteRenderer, id: i32, tint: Color32) {
    let size = if id > 0 {
        let ppp = ui.ctx().pixels_per_point();
        sr.rendered_sprite_size(id, TIP_ICON, ppp)
    } else {
        egui::vec2(TIP_ICON, TIP_ICON)
    };
    let (r, _) = ui.allocate_exact_size(size, Sense::hover());
    if id > 0 {
        sr.draw_sprite_in_rect_tinted(ui, id, r, tint);
    }
}

/// An icon immediately followed by its word(s), kept together on one line inside
/// a `horizontal_wrapped` block (a nested horizontal never splits), so a sprite
/// and its name never land on separate lines.
fn icon_word(ui: &mut egui::Ui, sr: &mut SpriteRenderer, id: i32, word: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        tip_icon(ui, sr, id);
        ui.label(RichText::new(word).strong());
    });
}

/// A tooltip header row: the objective's portal sprite (when known) + label.
fn tip_header_row(ui: &mut egui::Ui, sr: &mut SpriteRenderer, portal_id: i32, header: &str) {
    ui.horizontal(|ui| {
        if portal_id > 0 {
            tip_icon(ui, sr, portal_id);
        }
        ui.label(RichText::new(header).strong());
    });
}

/// Parse a mission difficulty target ("7-10", "5.5-10", or a single "5") into
/// an inclusive `[lo, hi]` range.
fn parse_difficulty_range(target: &str) -> Option<(f32, f32)> {
    let t = target.trim();
    if let Some((a, b)) = t.split_once('-') {
        let lo = a.trim().parse::<f32>().ok()?;
        let hi = b.trim().parse::<f32>().ok()?;
        Some((lo, hi))
    } else {
        let v = t.parse::<f32>().ok()?;
        Some((v, v))
    }
}

pub(crate) fn attach_dungeon_tooltip(
    resp: egui::Response,
    sr: &mut SpriteRenderer,
    name: &str,
    header: &str,
) -> egui::Response {
    // Building the tooltip contents (biome grouping, catalog folding, portal
    // lookups) is non-trivial, and this runs for every dungeon every frame.
    // Only do the work when this item is actually hovered (or its tooltip is
    // still open) -- the same condition egui uses to show the tooltip.
    if !(resp.contains_pointer() || resp.is_tooltip_open()) {
        return resp;
    }
    let name = name.to_string();
    let header = header.to_string();
    resp.hover_tip_ui(move |ui| render_dungeon_tip_body(ui, sr, &name, &header))
}

/// Render a dungeon's access/drop tooltip body directly into `ui` (no hover
/// wrapper), so it can be reused both as a hover tip on the mission cards and
/// inline inside the Live Feed Taskbar's mission tooltip.
pub(crate) fn render_dungeon_tip_body(
    ui: &mut egui::Ui,
    sr: &mut SpriteRenderer,
    name: &str,
    header: &str,
) {
    let low = name.trim().to_ascii_lowercase();
    // Item 7: Oryx's Sanctuary access requirements.
    if low == "oryx's sanctuary" {
        let self_portal = portal_sprite_id(name);
        let incant = item_sprite_id("Wine Cellar Incantation");
        let helm = item_sprite_id("Helmet Rune");
        let shield = item_sprite_id("Shield Rune");
        let sword = item_sprite_id("Sword Rune");
        tip_header_row(ui, sr, self_portal, header);
        ui.add_space(2.0);
        render_access_tokens(ui, sr, &oryx_sanctuary_access_tokens());
        ui.label(
            "Requires activating special items inside Oryx's Chamber and Wine Cellar after boss' deaths:",
        );
        bullet_row(ui, sr, &[incant], "Wine Cellar Incantation");
        bullet_row(ui, sr, &[helm], "Helmet Rune");
        bullet_row(ui, sr, &[shield], "Shield Rune");
        bullet_row(ui, sr, &[sword], "Sword Rune");
        return;
    }
    // Item 8: Void access requirements.
    if low == "void" || low == "the void" {
        let self_portal = portal_sprite_id(name);
        let sections = get_dungeon_drops().tooltip_model("Lost Halls");
        tip_header_row(ui, sr, self_portal, header);
        ui.add_space(2.0);
        render_access_tokens(ui, sr, &void_access_tokens());
        render_dungeon_sections(ui, sr, &sections);
        return;
    }
    // Oryx's Castle (Mark of Janus): a realm-close event, not a realm portal.
    if low == "oryx's castle" {
        let self_portal = portal_sprite_id(name);
        tip_header_row(ui, sr, self_portal, header);
        ui.add_space(2.0);
        ui.label(RichText::new(
            "Entered automatically about 2 minutes after Realm Score reaches 100%.",
        ));
        ui.add_space(2.0);
        let armor = enemy_sprite_id("Suit of Armor");
        let janus = enemy_sprite_id("Janus the Doorwarden");
        let guardians = enemy_sprite_id("Stone Guardian");
        ui.horizontal_wrapped(|ui| {
            ui.label("Clear all");
            icon_word(ui, sr, armor, "Suit of Armor");
            ui.label("in the castle to unlock");
            icon_word(ui, sr, janus, "Janus");
            ui.label("fight after");
            icon_word(ui, sr, guardians, "Stone Guardians.");
        });
        return;
    }
    // Oryx's Chamber (Mark of Oryx): reached by progressing through Oryx's Castle.
    if low == "oryx's chamber" {
        let self_portal = portal_sprite_id(name);
        let castle = portal_sprite_id("Oryx's Castle");
        tip_header_row(ui, sr, self_portal, header);
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("Accessed by progressing through");
            icon_word(ui, sr, castle, "Oryx's Castle");
            ui.label("after Realm Score reaches 100%.");
        });
        return;
    }
    // Wine Cellar (Trophy Hall portal): opened from inside Oryx's Chamber.
    if low == "wine cellar" {
        let self_portal = portal_sprite_id(name);
        let incant = item_sprite_id("Wine Cellar Incantation");
        let och = portal_sprite_id("Oryx's Chamber");
        tip_header_row(ui, sr, self_portal, header);
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("Accessed by using a");
            icon_word(ui, sr, incant, "Wine Cellar Incantation");
            ui.label("in");
            icon_word(ui, sr, och, "Oryx's Chamber");
            ui.label("after boss's death.");
        });
        return;
    }
    // Alien wormhole dungeons (Malogia/Untaris/Katalund/Forax) and their Neo
    // counterparts spawn as a fixed set of portals after a specific encounter is
    // defeated, rather than dropping from a spread of biome enemies.
    if let Some((biome_tier, encounter, in_biome)) = alien_dungeon_access(&low) {
        let self_portal = portal_sprite_id(name);
        let enc_id = enemy_sprite_id(encounter);
        tip_header_row(ui, sr, self_portal, header);
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("3");
            icon_word(ui, sr, self_portal, name);
            ui.label(format!(
                "portals will spawn in fixed locations in {biome_tier} Biomes"
            ));
            ui.label("after defeating");
            if let Some(biome) = in_biome {
                icon_word(ui, sr, enc_id, encounter);
                ui.label("in");
                icon_word(ui, sr, biome_beacon_sprite(biome), biome);
                ui.label(".");
            } else {
                icon_word(ui, sr, enc_id, &format!("{encounter}."));
            }
        });
        return;
    }
    if let Some(parts) = build_dungeon_parts(name) {
        let portal_id = portal_sprite_id(name);
        tip_header_row(ui, sr, portal_id, header);
        ui.add_space(2.0);
        render_tip_parts(ui, sr, &parts);
        return;
    }
    ui.label(RichText::new(header).strong());
}

/// One dungeon a mark quest points at, together with up to two of the best
/// regular-enemy drop sources in a given biome (used to fold overlapping
/// low-grave drops into shared "efficient areas").
pub(crate) struct AreaEntry {
    pub dungeon: String,
    /// Up to two resolvable `(sprite_id, name)` enemy sources for this dungeon
    /// within the biome, top-ranked first.
    pub monsters: Vec<(i32, String)>,
}

/// A biome that drops several of the quest's needed dungeons, with the best
/// enemy to farm for each.
pub(crate) struct AreaGroup {
    pub biome: String,
    pub entries: Vec<AreaEntry>,
}

/// The planned layout of a quest's drop tooltip: `efficient` biomes each cover
/// two or more needed low-grave dungeons (farm many marks in one place);
/// `individual` dungeons (grave 7+, or low-grave ones with no shared biome) keep
/// their own per-dungeon column.
pub(crate) struct DropTipPlan {
    pub efficient: Vec<AreaGroup>,
    pub individual: Vec<String>,
}

/// Rank a biome for stable ordering in the "efficient areas" list: by realm
/// tier (Veteran first) then name. Unknown-tier biomes sort last.
fn area_biome_rank(biome: &str) -> (u8, String) {
    let tier = get_dungeon_drops().biome_tier(biome);
    let t = tier.map_or(u8::MAX, |bt| match bt {
        BiomeTier::Veteran => 0,
        BiomeTier::Adept => 1,
        BiomeTier::Seasonal => 2,
        BiomeTier::Rookie => 3,
    });
    (t, biome.to_string())
}

/// Up to two best regular drop sources of `dungeon` per biome, from the scraped
/// drop model. Empty when the dungeon has no plain biome sources (e.g. a "found
/// inside X" special dungeon).
fn dungeon_biome_sources(dungeon: &str) -> Vec<(String, Vec<(i32, String)>)> {
    get_dungeon_drops()
        .tooltip_model(dungeon)
        .into_iter()
        .filter_map(|s| {
            let monsters: Vec<(i32, String)> = s
                .enemies
                .iter()
                .map(|n| (enemy_sprite_id(n), n.clone()))
                .filter(|(id, _)| *id != 0)
                .take(2)
                .collect();
            (!monsters.is_empty()).then_some((s.biome, monsters))
        })
        .collect()
}

/// Plan the drop tooltip for a quest's needed dungeons. Grave-7+ dungeons (rare
/// Veteran-encounter drops with little overlap) always render individually;
/// low-grave dungeons are folded into shared biomes wherever two or more of them
/// drop in the same place, so the player can farm several marks at once.
pub(crate) fn plan_drop_tip(dungeons: &[String]) -> DropTipPlan {
    let mut individual: Vec<String> = Vec::new();
    // dungeon -> its (biome, monsters) sources; only low-grave dungeons with
    // resolvable sources are eligible for folding.
    let mut sources: std::collections::HashMap<String, Vec<(String, Vec<(i32, String)>)>> =
        std::collections::HashMap::new();
    let mut remaining: Vec<String> = Vec::new();
    for d in dungeons {
        let high = dungeon_difficulty(d).is_some_and(|diff| diff >= 7.0);
        let biomes = if high {
            Vec::new()
        } else {
            dungeon_biome_sources(d)
        };
        if biomes.is_empty() {
            individual.push(d.clone());
        } else {
            sources.insert(d.clone(), biomes);
            remaining.push(d.clone());
        }
    }

    let mut efficient: Vec<AreaGroup> = Vec::new();
    // Repeatedly pull out the biome(s) covering the most still-unplaced dungeons.
    // Stop folding once the best coverage is a single dungeon (no overlap left):
    // those go to individual columns just like the grave-7+ set.
    while !remaining.is_empty() {
        // biome -> dungeons (in `remaining`) it drops, with the chosen monster.
        let mut coverage: std::collections::HashMap<String, Vec<AreaEntry>> =
            std::collections::HashMap::new();
        for d in &remaining {
            for (biome, monsters) in &sources[d] {
                coverage.entry(biome.clone()).or_default().push(AreaEntry {
                    dungeon: d.clone(),
                    monsters: monsters.clone(),
                });
            }
        }
        let best = coverage.values().map(|e| e.len()).max().unwrap_or(0);
        if best <= 1 {
            individual.extend(remaining.drain(..));
            break;
        }
        // Show every biome tied at the best coverage (capped) so the player has
        // a choice of efficient farming spots.
        let mut selected: Vec<(String, Vec<AreaEntry>)> = coverage
            .into_iter()
            .filter(|(_, e)| e.len() == best)
            .collect();
        selected.sort_by(|a, b| area_biome_rank(&a.0).cmp(&area_biome_rank(&b.0)));
        selected.truncate(4);

        let mut covered: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (biome, mut entries) in selected {
            entries.sort_by(|a, b| a.dungeon.cmp(&b.dungeon));
            for e in &entries {
                covered.insert(e.dungeon.clone());
            }
            efficient.push(AreaGroup { biome, entries });
        }
        remaining.retain(|d| !covered.contains(d));
    }

    DropTipPlan {
        efficient,
        individual,
    }
}

/// Render the "Most efficient areas to farm" block: one section per shared
/// biome, each listing `monster -> dungeon` rows so the player knows what to
/// kill and which mark it yields.
pub(crate) fn render_efficient_areas(
    ui: &mut egui::Ui,
    sr: &mut SpriteRenderer,
    groups: &[AreaGroup],
) {
    ui.label(RichText::new("Most efficient areas to farm:").strong());
    for (i, g) in groups.iter().enumerate() {
        if i > 0 {
            ui.separator();
        }
        biome_header_row(ui, sr, &g.biome);
        for e in &g.entries {
            let portal = portal_sprite_id(&e.dungeon);
            for (mid, mname) in &e.monsters {
                ui.horizontal(|ui| {
                    ui.add_space(6.0);
                    ui.label(RichText::new("\u{2022}").weak());
                    tip_icon(ui, sr, normalize_train_sprite(*mid, mname));
                    ui.label(RichText::new(mname).weak());
                    ui.label(RichText::new("\u{2192}").weak());
                    tip_icon(ui, sr, portal);
                    ui.label(RichText::new(&e.dungeon).weak());
                });
            }
        }
    }
}

fn attach_objective_tooltip(
    resp: egui::Response,
    sr: &mut SpriteRenderer,
    obj: &ObjectiveView,
    header: String,
) -> egui::Response {
    // Building the tooltip contents (biome grouping, catalog folding, portal
    // lookups) is non-trivial, and this runs for every objective every frame.
    // Only do the work when this row is actually hovered (or its tooltip is
    // still open) -- the same condition egui uses to show the tooltip.
    if !(resp.contains_pointer() || resp.is_tooltip_open()) {
        return resp;
    }
    let obj = obj.clone();
    resp.hover_tip_ui(move |ui| render_objective_tip_body(ui, sr, &obj, &header))
}

/// True when an objective has spawn/drop or qualifying-dungeon detail worth
/// showing (used to decide whether to append its section to the Taskbar
/// mission tooltip).
pub(crate) fn objective_has_spawn_info(obj: &ObjectiveView) -> bool {
    if encounter_beacon_group(&obj.target).is_some() {
        return true;
    }
    if matches!(obj.kind, ObjectiveKind::KillNamed)
        && obj
            .target
            .to_ascii_lowercase()
            .contains("twilight archmage")
    {
        return true;
    }
    if matches!(obj.kind, ObjectiveKind::KillNamed)
        && !get_dungeon_drops().encounter_biomes(&obj.target).is_empty()
    {
        return true;
    }
    if matches!(obj.kind, ObjectiveKind::Difficulty) {
        if let Some((lo, hi)) = parse_difficulty_range(&obj.target) {
            return !dungeons_in_difficulty_range(lo, hi).is_empty();
        }
    }
    if matches!(obj.kind, ObjectiveKind::Dungeon) {
        return obj.dungeon_name.is_some();
    }
    false
}

/// Render an objective's spawn/drop/qualifying-dungeon tooltip body directly
/// into `ui` (no hover wrapper), so it can be reused both as a hover tip on the
/// mission cards and inline inside the Live Feed Taskbar's mission tooltip.
pub(crate) fn render_objective_tip_body(
    ui: &mut egui::Ui,
    sr: &mut SpriteRenderer,
    obj: &ObjectiveView,
    header: &str,
) {
    if let Some(group) = encounter_beacon_group(&obj.target) {
        let entries = catalog_for_group(group);
        // Adept/Veteran encounter objectives keep the authoritative in-game
        // encounter membership but are grouped by biome for display. Beacon
        // Guardian stays a flat catalog (beacon sprites prepended).
        let biome_grouped = matches!(
            group,
            BossGroup::VeteranEncounter | BossGroup::AdeptEncounter
        );
        ui.label(RichText::new(header).strong());
        ui.add_space(2.0);
        if biome_grouped {
            render_encounter_catalog_grouped(ui, sr, &entries);
        } else {
            render_catalog_tooltip(ui, sr, &entries);
        }
        return;
    }
    // Item 9: named-boss kills that are Twilight Archmage list The Shatters'
    // drop sources.
    if matches!(obj.kind, ObjectiveKind::KillNamed)
        && obj
            .target
            .to_ascii_lowercase()
            .contains("twilight archmage")
    {
        let sections = get_dungeon_drops().tooltip_model("The Shatters");
        if !sections.is_empty() {
            let boss_id = enemy_sprite_id("Twilight Archmage");
            let portal_id = portal_sprite_id("The Shatters");
            tip_header_row(ui, sr, boss_id, header);
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Can be found in").strong());
                icon_word(ui, sr, portal_id, "The Shatters");
                ui.label(RichText::new("which drops from:").strong());
            });
            render_dungeon_sections(ui, sr, &sections);
            return;
        }
    }
    // A specific named-encounter boss (e.g. "Towering Perfection", "Cube
    // Deity") that the game shipped without a mission-card icon or spawn info:
    // show the boss sprite and the biome(s) its beacon appears in, recovered
    // from the bundled encounter data.
    if matches!(obj.kind, ObjectiveKind::KillNamed) {
        let biomes = get_dungeon_drops().encounter_biomes(&obj.target);
        if !biomes.is_empty() {
            let boss_id = match obj.boss_id {
                Some(id) if id > 0 => id,
                _ => enemy_sprite_id(&obj.target),
            };
            tip_header_row(ui, sr, boss_id, header);
            ui.add_space(2.0);
            let intro = if biomes.len() == 1 {
                "Appears in:"
            } else {
                "Appears in one of:"
            };
            ui.label(RichText::new(intro).strong());
            ui.add_space(2.0);
            for biome in &biomes {
                biome_header_row(ui, sr, biome);
            }
            return;
        }
    }
    // Item 1: grave-difficulty-range objectives list every qualifying dungeon.
    if matches!(obj.kind, ObjectiveKind::Difficulty) {
        if let Some((lo, hi)) = parse_difficulty_range(&obj.target) {
            let rows: Vec<(String, i32)> = dungeons_in_difficulty_range(lo, hi)
                .into_iter()
                .map(|(n, _)| (n.to_string(), portal_sprite_id(n)))
                .collect();
            if !rows.is_empty() {
                ui.label(RichText::new("Dungeons that will count for this mission:").strong());
                ui.add_space(2.0);
                let groups = rows
                    .into_iter()
                    .map(|(name, id)| TipGroup {
                        separated: false,
                        rows: vec![TipCell::Bullet {
                            sprite_ids: vec![id],
                            name,
                            note: None,
                        }],
                    })
                    .collect();
                flow_column_groups(ui, sr, groups);
                return;
            }
        }
    }
    if matches!(obj.kind, ObjectiveKind::Dungeon) {
        if let Some(name) = &obj.dungeon_name {
            render_dungeon_tip_body(ui, sr, name, header);
            return;
        }
    }
    ui.label(RichText::new(header).strong());
}

/// One objective tile (dungeon portal or boss sprite), dimmed when greyed or not
/// yet `lit`. `dy` nudges the drawn tile vertically (negative = up) without
/// moving layout. Objectives without a resolvable sprite draw no tile.
/// Portal ids whose art fills the full 16px frame, so integer scaling renders
/// them a step smaller than dome-style portals (The Nest, Plagued Nest) in the
/// same objective cell. Drawn fit-to-cell so their height matches. Currently the
/// Kogbold Steamworks portals (regular + Advanced).
fn is_full_frame_portal(id: i32) -> bool {
    matches!(id, 28822 | 49433)
}

pub(crate) fn draw_obj_tile(
    ui: &mut egui::Ui,
    sr: &mut SpriteRenderer,
    icon: &ObjIcon,
    lit: bool,
    greyed: bool,
    dy: f32,
) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(OBJ_ICON, OBJ_ICON), Sense::hover());
    let drect = rect.translate(egui::vec2(0.0, dy));
    // Darken the sprite by tint (alpha-masked) rather than a covering rect.
    let tint = if greyed {
        Color32::from_gray(80)
    } else if !lit {
        Color32::from_gray(120)
    } else {
        Color32::WHITE
    };
    paint_obj_icon(ui, sr, icon, drect, tint);
    drect
}

/// Paint an [`ObjIcon`] into an arbitrary `rect` with the given `tint`, without
/// allocating layout space. Shared by the mission-card tiles and the Live Feed
/// Taskbar pills.
pub(crate) fn paint_obj_icon(
    ui: &mut egui::Ui,
    sr: &mut SpriteRenderer,
    icon: &ObjIcon,
    rect: egui::Rect,
    tint: Color32,
) {
    match icon {
        ObjIcon::Object(id) if *id > 0 => {
            if is_full_frame_portal(*id) || sr.sprite_exceeds_cell(*id, ui, rect) {
                sr.draw_outlined_sprite_in_rect_fit_tinted(ui, *id, rect, tint);
            } else {
                sr.draw_outlined_sprite_in_rect_tinted(ui, *id, rect, tint);
            }
        }
        ObjIcon::Embedded(e) => {
            sr.draw_embedded_icon_tinted(ui, *e, rect, tint);
        }
        ObjIcon::Sheet { sheet, idx } => {
            sr.draw_sprite_by_sheet_tinted(ui, sheet, *idx, rect, tint);
        }
        ObjIcon::Object(_) | ObjIcon::None => {}
    }
}

/// True when an AND mission's objectives are all small-count (1-3) and every one
/// resolves to an icon (dungeon portal or boss sprite) -- rendered as one compact
/// horizontal row of icons instead of a stack of bars (the description subtitle
/// already conveys "complete all").
fn is_compact_icon_and(e: &MissionEntryView) -> bool {
    !e.one_of
        && e.objectives.len() > 1
        && e.objectives
            .iter()
            .all(|o| o.need >= 1 && o.need <= 3 && obj_icon_id(o) > 0)
        && e.objectives.iter().map(|o| o.need).sum::<i32>() <= 10
}

/// Render an AND group of small-count objectives as a single horizontal row of
/// icon tiles (each objective repeated `need` times, lit as completed).
fn compact_icon_row(
    ui: &mut egui::Ui,
    sr: &mut SpriteRenderer,
    objectives: &[ObjectiveView],
    mission_icon: &ObjIcon,
    greyed: bool,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = OBJ_ICON_GAP;
        // Reserve the same leading icon slot as bar rows so the tiles line up on
        // the progress-bar start line (and under the description text).
        ui.allocate_exact_size(egui::vec2(OBJ_ICON, OBJ_ICON), Sense::hover());
        for obj in objectives {
            let icon = obj_icon(obj, mission_icon);
            let need = obj.need.max(1);
            for i in 0..need {
                let rect = draw_obj_tile(ui, sr, &icon, i < obj.have, greyed, 0.0);
                let counter = format!("  {}/{}", obj.have.min(need), need);
                let resp = ui.interact(rect, ui.id().with((obj.label.as_str(), i)), Sense::hover());
                attach_objective_tooltip(resp, sr, obj, format!("{}{}", obj.label, counter));
            }
        }
    });
}

/// Render one objective as an in-game-style progress bar: a target/portal icon,
/// a filled bar, and a `have/need` count. Adds an OR/AND connector when not last.
fn objective_line(
    ui: &mut egui::Ui,
    sr: &mut SpriteRenderer,
    obj: &ObjectiveView,
    mission_icon: &ObjIcon,
    greyed: bool,
    connector: Option<&str>,
    attach_tip: bool,
) {
    let resp = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = OBJ_ICON_GAP;
            // Optical alignment: the bar sits on the row baseline.
            let dy = 0.0;
            // Always reserve the icon slot so bars align on one vertical line;
            // every objective now resolves to an icon (dungeon portal, boss,
            // gravestone, encounter marker, or the mission's own icon).
            draw_obj_tile(ui, sr, &obj_icon(obj, mission_icon), true, greyed, dy);

            let need = obj.need.max(1);
            let have = obj.have.clamp(0, need);
            let frac = have as f32 / need as f32;
            let done = have >= need;

            let (brect0, _) = ui.allocate_exact_size(egui::vec2(BAR_W, BAR_H), Sense::hover());
            let brect = brect0.translate(egui::vec2(0.0, dy));
            let painter = ui.painter();
            painter.rect_filled(brect, 4.0, Color32::from_gray(34));
            if frac > 0.0 {
                let fw = (brect.width() * frac).max(4.0);
                let fill = egui::Rect::from_min_size(brect.min, egui::vec2(fw, brect.height()));
                let col = if greyed {
                    Color32::from_gray(95)
                } else if done {
                    Color32::from_rgb(238, 168, 40)
                } else {
                    Color32::from_rgb(74, 149, 199)
                };
                painter.rect_filled(fill, 4.0, col);
            }
            painter.rect_stroke(
                brect,
                4.0,
                Stroke::new(1.0_f32, Color32::from_gray(58)),
                StrokeKind::Inside,
            );
            let count = format!("{have}/{need}");
            let font = FontId::proportional(11.5);
            let outline = Color32::from_black_alpha(if greyed { 120 } else { 230 });
            for dx in [-1.0f32, 0.0, 1.0] {
                for dy in [-1.0f32, 0.0, 1.0] {
                    if dx == 0.0 && dy == 0.0 {
                        continue;
                    }
                    painter.text(
                        brect.center() + egui::vec2(dx, dy),
                        egui::Align2::CENTER_CENTER,
                        &count,
                        font.clone(),
                        outline,
                    );
                }
            }
            painter.text(
                brect.center(),
                egui::Align2::CENTER_CENTER,
                &count,
                font,
                if greyed {
                    Color32::from_gray(170)
                } else {
                    Color32::WHITE
                },
            );

            if let Some(c) = connector {
                ui.add_space(3.0);
                ui.label(
                    RichText::new(c)
                        .size(11.0)
                        .strong()
                        .color(Color32::from_gray(if greyed { 110 } else { 165 })),
                );
            }
        })
        .response;

    let counter = if obj.need > 0 {
        format!("  {}/{}", obj.have.min(obj.need), obj.need)
    } else {
        String::new()
    };
    if attach_tip {
        attach_objective_tooltip(resp, sr, obj, format!("{}{}", obj.label, counter));
    }
}

/// Render a single item reward: a sprite tile followed by `x N` when stacked.
fn reward_chip(ui: &mut egui::Ui, sr: &mut SpriteRenderer, r: &RewardView, greyed: bool) {
    match r {
        RewardView::Item {
            object_id,
            amount,
            name,
        } => {
            let (rect, resp) =
                ui.allocate_exact_size(egui::vec2(REWARD_TILE, REWARD_TILE), Sense::hover());
            if *object_id > 0 {
                if greyed {
                    sr.draw_outlined_sprite_in_rect_tinted(
                        ui,
                        *object_id,
                        rect,
                        Color32::from_gray(90),
                    );
                } else {
                    sr.draw_outlined_sprite_in_rect(ui, *object_id, rect);
                }
            }
            let tip = if *amount > 1 {
                format!("{name} x{amount}")
            } else {
                name.clone()
            };
            resp.hover_tip(tip);
            if *amount > 1 {
                let c = if greyed {
                    Color32::from_gray(140)
                } else {
                    Color32::from_gray(220)
                };
                ui.label(RichText::new(format!("x{amount}")).size(11.5).color(c));
            }
        }
        RewardView::Bxp(n) => draw_bxp(ui, *n, greyed),
    }
}

/// Battle-pass XP in the in-game style: white number + heavy-bold green "BXP".
fn draw_bxp(ui: &mut egui::Ui, n: i64, greyed: bool) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        let (num_c, bxp_c) = if greyed {
            (Color32::from_gray(150), Color32::from_gray(120))
        } else {
            (Color32::WHITE, Color32::from_rgb(126, 190, 60))
        };
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format_bxp_num(n))
                    .size(13.0)
                    .strong()
                    .color(num_c),
            );
            ui.label(RichText::new("BXP").size(13.0).strong().color(bxp_c))
                .hover_tip(format!("{} battle-pass XP", space_thousands(n)));
        });
    });
}

/// Render all reward groups on a single line: BXP first (always an AND reward),
/// then item groups joined by `+`; items inside a pick-one group joined by `or`.
fn rewards_line(ui: &mut egui::Ui, sr: &mut SpriteRenderer, groups: &[RewardGroup], greyed: bool) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let sep_col = if greyed {
            Color32::from_gray(110)
        } else {
            Color32::from_gray(180)
        };

        // BXP entries first (AND to everything else).
        let mut any_bxp = false;
        for g in groups {
            for r in &g.rewards {
                if let RewardView::Bxp(n) = r {
                    if any_bxp {
                        ui.add_space(REWARD_PLUS_PAD);
                        ui.label(RichText::new("+").size(13.0).strong().color(sep_col));
                        ui.add_space(REWARD_PLUS_PAD);
                    }
                    draw_bxp(ui, *n, greyed);
                    any_bxp = true;
                }
            }
        }

        let item_groups: Vec<&RewardGroup> = groups
            .iter()
            .filter(|g| {
                g.rewards
                    .iter()
                    .any(|r| matches!(r, RewardView::Item { .. }))
            })
            .collect();
        if item_groups.is_empty() {
            return;
        }

        // The "+" bridging BXP and the item groups stays on the BXP baseline.
        if any_bxp {
            ui.add_space(REWARD_PLUS_PAD);
            ui.label(RichText::new("+").size(13.0).strong().color(sep_col));
            ui.add_space(REWARD_PLUS_PAD);
        }

        // Item groups render in a child nudged 4px up (optical alignment) while
        // still reserving their footprint in the parent row.
        let start = ui.cursor().min;
        let avail = ui.available_width().max(0.0);
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(egui::Rect::from_min_size(
                    start + egui::vec2(0.0, -4.0),
                    egui::vec2(avail, REWARD_TILE),
                ))
                .layout(Layout::left_to_right(Align::Center)),
        );
        child.spacing_mut().item_spacing.x = 6.0;
        for (gi, g) in item_groups.iter().enumerate() {
            if gi > 0 {
                child.add_space(REWARD_PLUS_PAD);
                child.label(RichText::new("+").size(13.0).strong().color(sep_col));
                child.add_space(REWARD_PLUS_PAD);
            }
            let items: Vec<&RewardView> = g
                .rewards
                .iter()
                .filter(|r| matches!(r, RewardView::Item { .. }))
                .collect();
            // Bracket a multiple-choice group so its "or"-linked items read as a
            // single grouped selection.
            let bracketed = g.one_of && items.len() > 1;
            let sep = if g.one_of { "or" } else { "+" };
            if bracketed {
                child.label(RichText::new("[").size(15.0).strong().color(sep_col));
            }
            for (i, r) in items.iter().enumerate() {
                if i > 0 {
                    child.label(RichText::new(sep).size(11.5).italics().color(sep_col));
                }
                reward_chip(&mut child, sr, r, greyed);
            }
            if bracketed {
                child.label(RichText::new("]").size(15.0).strong().color(sep_col));
            }
        }
        let used = child.min_rect().width();
        ui.allocate_space(egui::vec2(used, REWARD_TILE));
    });
}

/// Draw a small almond-shaped eye (outline + pupil) centred at `c`, `w` wide.
fn draw_eye(p: &egui::Painter, c: egui::Pos2, w: f32, color: Color32) {
    let h = w * 0.62;
    let n = 12;
    let mut pts = Vec::with_capacity(2 * (n + 1));
    for i in 0..=n {
        let x = -w / 2.0 + w * (i as f32 / n as f32);
        let y = -(h / 2.0) * (1.0 - (2.0 * x / w).powi(2));
        pts.push(c + egui::vec2(x, y));
    }
    for i in (0..=n).rev() {
        let x = -w / 2.0 + w * (i as f32 / n as f32);
        let y = (h / 2.0) * (1.0 - (2.0 * x / w).powi(2));
        pts.push(c + egui::vec2(x, y));
    }
    p.add(egui::Shape::closed_line(pts, Stroke::new(1.2_f32, color)));
    p.circle_filled(c, h * 0.30, color);
}

/// A collapsible status-section header ("▼ In progress  (3)"). Returns true when
/// the header was clicked (toggle the section's collapsed state).
fn section_header(
    ui: &mut egui::Ui,
    expanded: bool,
    title: &str,
    color: Color32,
    count: usize,
) -> bool {
    let arrow = if expanded { "▼" } else { "▶" };
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.add_space(2.0);
        if ui
            .selectable_label(
                false,
                RichText::new(format!("{arrow}  {title}"))
                    .strong()
                    .size(14.0)
                    .color(color),
            )
            .clicked()
        {
            clicked = true;
        }
        ui.label(
            RichText::new(format!("({count})"))
                .size(12.0)
                .color(Color32::from_gray(130)),
        );
    });
    clicked
}

/// Fixed-size column cell (`w` x `h`) whose content is vertically centred on the
/// card's middle axis via a `left_to_right(Center)` inner layout. The width and
/// height are always reserved (even when empty) to keep columns aligned. Content
/// may overflow the width (e.g. a long reward line) without disturbing layout;
/// multi-row content should wrap itself in `ui.vertical(..)` so it centres as a
/// single block.
fn cell(ui: &mut egui::Ui, w: f32, h: f32, add: impl FnOnce(&mut egui::Ui)) {
    cell_off(ui, w, h, 0.0, add);
}

/// Like [`cell`] but nudges its content vertically by `dy` px (negative = up),
/// for per-column optical alignment without changing the reserved layout.
fn cell_off(ui: &mut egui::Ui, w: f32, h: f32, dy: f32, add: impl FnOnce(&mut egui::Ui)) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), Sense::hover());
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(rect.translate(egui::vec2(0.0, dy)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    add(&mut child);
}

fn header_row(ui: &mut egui::Ui, name_w: f32, reward_w: f32, hide_name_icon: bool) {
    let lbl = |s: &str| {
        RichText::new(s)
            .strong()
            .size(12.0)
            .color(Color32::from_gray(170))
    };
    let h = 16.0;
    ui.horizontal(|ui| {
        ui.add_space(PAD_X);
        ui.spacing_mut().item_spacing.x = COL_GAP;
        if !hide_name_icon {
            cell(ui, W_ICON, h, |_| {});
        }
        cell(ui, W_TYPE, h, |ui| {
            ui.label(lbl("Type"));
        });
        cell(ui, W_STATUS, h, |ui| {
            ui.label(lbl("Status"));
        });
        if !hide_name_icon {
            ui.spacing_mut().item_spacing.x = NAME_GAP;
            cell(ui, name_w, h, |ui| {
                ui.label(lbl("Mission"));
            });
            ui.spacing_mut().item_spacing.x = COL_GAP;
        }
        cell(ui, W_OBJ, h, |ui| {
            ui.label(lbl("Objective"));
        });
        cell(ui, W_ARROW, h, |_| {});
        cell(ui, reward_w, h, |ui| {
            ui.label(lbl("Reward"));
        });
    });
    ui.add_space(2.0);
}

/// Draw the Live Feed Taskbar "track" compass toggle at `rect`. Bright with a
/// yellow outline when the source is tracked, dimmed with a subtle outline when
/// not. Returns the click response; the caller flips the tracking state and
/// attaches the tooltip.
pub(crate) fn taskbar_compass_button(
    ui: &mut egui::Ui,
    sr: &mut SpriteRenderer,
    rect: egui::Rect,
    id: egui::Id,
    tracked: bool,
) -> egui::Response {
    let resp = ui.interact(rect, id, Sense::click());
    let hovered = resp.hovered();
    let gold = Color32::from_rgb(235, 205, 70);
    let bg = if tracked {
        Color32::from_rgba_unmultiplied(70, 62, 20, 205)
    } else if hovered {
        Color32::from_black_alpha(130)
    } else {
        Color32::from_black_alpha(80)
    };
    ui.painter().rect_filled(rect, 4.0, bg);
    let tint = if tracked || hovered {
        Color32::WHITE
    } else {
        Color32::from_white_alpha(120)
    };
    sr.draw_embedded_icon_tinted(ui, EmbeddedIcon::Compass, rect.shrink(3.0), tint);
    let stroke = if tracked {
        Stroke::new(1.5_f32, gold)
    } else if hovered {
        Stroke::new(1.0_f32, Color32::from_gray(120))
    } else {
        Stroke::new(1.0_f32, Color32::from_black_alpha(110))
    };
    ui.painter()
        .rect_stroke(rect, 4.0, stroke, StrokeKind::Inside);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Render a single mission as an aligned card row. Pushes to `to_hide` on Hide.
fn mission_card(
    ui: &mut egui::Ui,
    sr: &mut SpriteRenderer,
    e: &MissionEntryView,
    card_fill: Color32,
    name_w: f32,
    reward_w: f32,
    dragged: bool,
    hide_name_icon: bool,
    current_char: Option<CurrentChar>,
    to_hide: &mut Vec<i64>,
    to_unhide: &mut Vec<i64>,
    in_hidden: bool,
    toggled_cat: &mut Option<u8>,
    show_compass: bool,
    tracked: bool,
    toggle_track: &mut Option<i64>,
    show_tree: bool,
) -> egui::Response {
    let claimed = e.state == MissionState::Claimed;
    let locked = e.state == MissionState::Locked;
    let cooldown = on_cooldown(e);
    let claimable = is_claimable(e) && !claimed;
    // Any claimed mission (done forever or waiting on cooldown) is muted with a
    // single card-shaped overlay drawn over its content (below), rather than
    // dimming each element -- the character sprite draws without a tint, so
    // per-element greying left it at full brightness among faded siblings. A
    // locked mission is muted the same way.
    let muted = claimed || locked;
    // Content renders at full brightness; the overlay mutes it uniformly.
    let greyed = false;

    let has_desc = !e.desc.is_empty();
    let has_worn = e.worn_restriction.iter().any(|w| !w.display.is_empty());
    let compact = is_compact_icon_and(e);
    let obj_rows = if compact { 1 } else { e.objectives.len() };
    // Wrap the description across the objective column; taller cards get the
    // extra rows so nothing is clipped.
    let desc_lines = if has_desc {
        ui.painter()
            .layout(
                e.desc.clone(),
                FontId::proportional(12.5),
                Color32::WHITE,
                W_OBJ - OBJ_INDENT,
            )
            .rows
            .len()
            .max(1)
    } else {
        0
    };
    let obj_lines = obj_rows + has_desc as usize + has_worn as usize;
    let reward_lines = !e.reward_groups.is_empty() as usize;
    let n_lines = obj_lines.max(reward_lines).max(1) as f32;
    let extra_desc = desc_lines.saturating_sub(1) as f32;
    // Always reserve room for the tag + character sprite/indicator stack so the
    // tag stays in its final (top-aligned) position even before a live
    // character is resolved; the sprite/indicator simply fills in later.
    let type_block_h = 48.0;
    let content_h = (n_lines * LINE_H + extra_desc * DESC_ROW_H)
        .max(BASE_H)
        .max(type_block_h);
    let card_h = content_h + PAD_Y * 2.0;

    let avail = ui.available_width();
    let (card_rect, resp) =
        ui.allocate_exact_size(egui::vec2(avail, card_h), Sense::click_and_drag());
    let fill = if dragged {
        Color32::from_rgba_unmultiplied(card_fill.r(), card_fill.g(), card_fill.b(), 150)
    } else {
        card_fill
    };
    ui.painter().rect_filled(card_rect, 6.0, fill);
    // A completed, unclaimed mission is actionable now: a soft yellow glow plus
    // a crisp outline so it stands out at a glance.
    if claimable {
        let gold = Color32::from_rgb(235, 205, 70);
        let p = ui.painter();
        p.rect_stroke(
            card_rect.expand(1.0),
            7.0,
            Stroke::new(
                3.0_f32,
                Color32::from_rgba_unmultiplied(gold.r(), gold.g(), gold.b(), 26),
            ),
            StrokeKind::Outside,
        );
        p.rect_stroke(
            card_rect,
            6.0,
            Stroke::new(2.0_f32, gold),
            StrokeKind::Inside,
        );
    }

    let content_rect = egui::Rect::from_min_max(
        card_rect.min + egui::vec2(PAD_X, PAD_Y),
        card_rect.max - egui::vec2(PAD_X, PAD_Y),
    );
    let mut cui = ui.new_child(
        UiBuilder::new()
            .max_rect(content_rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    cui.spacing_mut().item_spacing.x = COL_GAP;

    // Mission icon: unique in-game icon (sheet sprite) if available, else the
    // reward-sprite fallback. Tooltip = description.
    if !hide_name_icon {
        cell(&mut cui, W_ICON, content_h, |ui| {
            let (irect, iresp) =
                ui.allocate_exact_size(egui::vec2(MISSION_ICON, MISSION_ICON), Sense::hover());
            let drawn = if let Some((sheet, idx)) = &e.icon_ref {
                let tint = if greyed {
                    Color32::from_gray(90)
                } else {
                    Color32::WHITE
                };
                sr.draw_sprite_by_sheet_tinted(ui, sheet, *idx, irect, tint)
            } else {
                false
            };
            if !drawn {
                if let Some(id) = e.icon_object_id {
                    if greyed {
                        sr.draw_outlined_sprite_in_rect_tinted(
                            ui,
                            id,
                            irect,
                            Color32::from_gray(90),
                        );
                    } else {
                        sr.draw_outlined_sprite_in_rect(ui, id, irect);
                    }
                }
            }
            if !e.desc.is_empty() {
                iresp.hover_tip(e.desc.clone());
            }
        });
    }

    // Type badge with the live character's sprite + qualify/deny indicator
    // stacked beneath it, vertically centred on the card's middle axis (a nested
    // top-down inside `cell` would otherwise glue the group to the top and look
    // unbalanced on tall multi-objective cards).
    {
        const BADGE_H: f32 = 18.0;
        let (rect, _) = cui.allocate_exact_size(egui::vec2(W_TYPE, content_h), Sense::hover());
        let block_h = if current_char.is_some() {
            BADGE_H + 4.0 + 22.0
        } else {
            BADGE_H
        };
        let inner = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.center().y - block_h / 2.0),
            egui::vec2(W_TYPE, block_h),
        );
        let mut tui = cui.new_child(
            UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::top_down(Align::Center)),
        );
        tui.spacing_mut().item_spacing.y = 4.0;
        if type_badge(&mut tui, e.category, greyed).clicked() {
            *toggled_cat = Some(e.category.order());
        }
        if let Some(c) = current_char {
            draw_char_qualifier(
                &mut tui,
                sr,
                &c,
                e.participants,
                &e.worn_restriction,
                greyed,
            );
        }
    }

    // Status + repeatability (merged column). Status text on top, reset/repeat
    // info below it -- rendered in a vertically-centred child so the group sits
    // on the card's middle axis (a nested vertical inside `cell` top-aligns).
    {
        let (rect, _) = cui.allocate_exact_size(egui::vec2(W_STATUS, content_h), Sense::hover());
        let block_h = if e.repeatable { 35.0 } else { 17.0 };
        let inner = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.center().y - block_h / 2.0),
            egui::vec2(W_STATUS, block_h),
        );
        let mut sui = cui.new_child(
            UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::top_down(Align::Min)),
        );
        let ui = &mut sui;
        let yellow = Color32::from_rgb(235, 205, 70);
        ui.spacing_mut().item_spacing.y = 2.0;
        let (text, color, tip) = if claimable {
            ("Claimable", yellow, "Completed - reward ready to claim")
        } else if claimed {
            (
                "Claimed",
                Color32::from_rgb(110, 190, 90),
                "Completed and claimed",
            )
        } else if locked {
            (
                "Locked",
                Color32::from_gray(150),
                "Locked - complete earlier missions in this tree to unlock",
            )
        } else {
            (
                "In progress",
                Color32::from_rgb(74, 149, 199),
                "In progress",
            )
        };
        ui.label(RichText::new(text).size(12.5).strong().color(color))
            .hover_tip(tip);

        if e.repeatable {
            let grey = Color32::from_gray(120);
            let (glyph_txt, glyph_col, txt_col, tip): (String, Color32, Color32, String) =
                if cooldown {
                    let rem = e.reset_at.map(|r| r - now_unix());
                    match rem {
                        Some(s) if s > 0 => {
                            // Repaint at the reset boundary (capped at 30s so the
                            // countdown label still ticks) so an elapsed mission
                            // jumps to "In progress" on time.
                            ui.ctx()
                                .request_repaint_after(std::time::Duration::from_secs(
                                    s.clamp(1, 30) as u64,
                                ));
                            (
                                format!("Resets {}", fmt_countdown_short(s)),
                                grey,
                                grey,
                                format!("Repeatable - restarts in {}", fmt_countdown_long(s)),
                            )
                        }
                        Some(_) => (
                            "Resets soon".to_string(),
                            grey,
                            grey,
                            "Repeatable - reset due (press Refresh to update)".to_string(),
                        ),
                        None => (
                            "Resets --:--".to_string(),
                            grey,
                            grey,
                            "Repeatable - on cooldown (reset time not yet known)".to_string(),
                        ),
                    }
                } else {
                    (
                        "Repeatable".to_string(),
                        yellow,
                        yellow,
                        "Repeatable mission".to_string(),
                    )
                };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                ui.label(RichText::new("⟳").size(13.0).color(glyph_col));
                ui.label(RichText::new(glyph_txt).size(11.0).color(txt_col));
            })
            .response
            .hover_tip(tip);
        }
    }

    // Mission name (always fully visible, Combat-History title size). The gap
    // before it is tightened so wide names have more room.
    if !hide_name_icon {
        cui.spacing_mut().item_spacing.x = NAME_GAP;
        cell(&mut cui, name_w, content_h, |ui| {
            let col = if greyed {
                Color32::from_gray(140)
            } else {
                Color32::WHITE
            };
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                let resp = ui.label(RichText::new(&e.name).size(NAME_SIZE).strong().color(col));
                if !e.desc.is_empty() {
                    resp.hover_tip(e.desc.clone());
                }
                // Tree label: only when multiple trees are merged, so single-
                // tree battlepasses stay uncluttered.
                if show_tree && !e.tree_name.is_empty() {
                    ui.label(
                        RichText::new(&e.tree_name)
                            .size(11.0)
                            .color(Color32::from_gray(140)),
                    );
                }
            });
        });
        cui.spacing_mut().item_spacing.x = COL_GAP;
    }

    // Objective: grey description subtitle, then progress bars -- or a single
    // compact portal row for AND groups of small-count dungeons.
    cell(&mut cui, W_OBJ, content_h, |ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            if has_desc {
                let dcol = if greyed {
                    Color32::from_gray(110)
                } else {
                    Color32::from_gray(150)
                };
                // Indent to match the progress-bar start so text and bars align.
                ui.horizontal(|ui| {
                    ui.add_space(OBJ_INDENT);
                    ui.add(egui::Label::new(RichText::new(&e.desc).size(12.5).color(dcol)).wrap());
                });
            }
            if has_worn {
                let names: Vec<&str> = e
                    .worn_restriction
                    .iter()
                    .map(|w| w.display.as_str())
                    .filter(|s| !s.is_empty())
                    .collect();
                let col = if greyed {
                    Color32::from_gray(120)
                } else {
                    Color32::from_rgb(235, 205, 70)
                };
                ui.horizontal(|ui| {
                    ui.add_space(OBJ_INDENT);
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("Requires {} equipped", names.join(" or ")))
                                .size(12.0)
                                .italics()
                                .color(col),
                        )
                        .wrap(),
                    );
                });
            }
            let mission_icon = mission_fallback_icon(e);
            if compact {
                compact_icon_row(ui, sr, &e.objectives, &mission_icon, greyed);
            } else {
                let n = e.objectives.len();
                let connector = if e.one_of { "OR" } else { "AND" };
                // Single-objective cards attach the tooltip to the whole card
                // below; multi-objective cards keep a per-bar tooltip so each
                // dungeon/encounter stays individually addressable.
                let per_bar_tip = n > 1;
                for (i, obj) in e.objectives.iter().enumerate() {
                    let c = (n > 1 && i + 1 < n).then_some(connector);
                    objective_line(ui, sr, obj, &mission_icon, greyed, c, per_bar_tip);
                }
            }
        });
    });

    // Arrow column (aligned).
    cell_off(&mut cui, W_ARROW, content_h, -1.0, |ui| {
        ui.label(
            RichText::new("→")
                .strong()
                .size(15.0)
                .color(Color32::from_gray(if greyed { 110 } else { 200 })),
        );
    });

    // Rewards: single line -- BXP first, groups joined by "+", pick-one by "or".
    cell_off(&mut cui, reward_w, content_h, 0.0, |ui| {
        rewards_line(ui, sr, &e.reward_groups, greyed);
    });

    // Mute the whole card with one card-shaped semi-transparent overlay, so the
    // character sprite (drawn without a tint) greys out alongside everything
    // else. Drawn before the Hide button / hover outline so those stay crisp.
    if muted {
        ui.painter()
            .rect_filled(card_rect, 6.0, Color32::from_black_alpha(120));
    }

    // Gift badge in the card's top-left corner, marking a claimable reward
    // (mirrors the in-game claimable icon, which RotMG places in the corner).
    if claimable {
        let g = 22.0;
        let gift_rect = egui::Rect::from_min_size(
            egui::pos2(card_rect.left() - 4.0, card_rect.top() - 4.0),
            egui::vec2(g, g),
        );
        sr.draw_embedded_icon(ui, EmbeddedIcon::Gift, gift_rect);
    }

    // Hide / Unhide button, top-right corner of the card. In the "Hidden by
    // user" section it becomes "Unhide" (return-arrow glyph) to undo; otherwise
    // a labelled "Hide" (eye glyph, not a bare ✕ which reads as delete) makes it
    // clear the mission is recoverable from that section.
    let font = FontId::proportional(11.5);
    let label = if in_hidden { "Unhide" } else { "Hide" };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font.clone(), Color32::WHITE);
    let icon_w = 14.0;
    let inner = egui::vec2(icon_w + 3.0 + galley.size().x, 15.0);
    let pad = egui::vec2(6.0, 3.0);
    let size = inner + pad * 2.0;
    let btn_rect = egui::Rect::from_min_size(
        egui::pos2(card_rect.right() - 8.0 - size.x, card_rect.top() + 6.0),
        size,
    );
    let btn = if in_hidden {
        ui.interact(btn_rect, ui.id().with(("unhide", e.uid())), Sense::click())
    } else {
        ui.interact(btn_rect, ui.id().with(("hide", e.uid())), Sense::click())
    };
    let btn_clicked = btn.clicked();
    let (bg, fg) = if btn.hovered() {
        (Color32::from_gray(64), Color32::from_gray(235))
    } else {
        (Color32::from_gray(40), Color32::from_gray(155))
    };
    let p = ui.painter();
    p.rect_filled(btn_rect, 4.0, bg);
    let icon_c = egui::pos2(btn_rect.left() + pad.x + icon_w * 0.5, btn_rect.center().y);
    if in_hidden {
        p.text(
            icon_c,
            egui::Align2::CENTER_CENTER,
            "\u{27f2}",
            FontId::proportional(14.0),
            fg,
        );
    } else {
        draw_eye(p, icon_c, icon_w, fg);
    }
    p.text(
        egui::pos2(btn_rect.left() + pad.x + icon_w + 3.0, btn_rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        fg,
    );
    let tip = if in_hidden {
        "Unhide this mission"
    } else {
        "Temporarily hide this mission (find it under \"Hidden by user\")"
    };
    btn.on_hover_cursor(egui::CursorIcon::PointingHand)
        .hover_tip(tip);
    if btn_clicked {
        if in_hidden {
            to_unhide.push(e.uid());
        } else {
            to_hide.push(e.uid());
        }
    }

    // Compass toggle (left of Hide) to add/remove this in-progress mission from
    // the Live Feed Taskbar. Only shown for missions eligible to appear there.
    if show_compass {
        let cs = 20.0;
        let comp_rect = egui::Rect::from_min_size(
            egui::pos2(btn_rect.left() - 5.0 - cs, btn_rect.center().y - cs * 0.5),
            egui::vec2(cs, cs),
        );
        let r = taskbar_compass_button(
            ui,
            sr,
            comp_rect,
            ui.id().with(("taskbar_compass_m", e.uid())),
            tracked,
        );
        let tip = if tracked {
            "Tracked in the Taskbar. Click to remove."
        } else {
            "Click to add this mission to your Taskbar"
        };
        if r.hover_tip(tip).clicked() {
            *toggle_track = Some(e.uid());
        }
    }

    // Blue outline on hover (or while dragged), matching Loot/Combat History
    // cards. Skipped for claimable cards so their yellow outline stays dominant.
    if dragged {
        ui.painter().rect_stroke(
            card_rect,
            6.0,
            Stroke::new(2.0_f32, Color32::from_rgb(100, 150, 255)),
            StrokeKind::Inside,
        );
    } else if !claimable && ui.rect_contains_pointer(card_rect) {
        ui.painter().rect_stroke(
            card_rect,
            6.0,
            Stroke::new(1.5_f32, Color32::from_rgb(90, 130, 170)),
            StrokeKind::Inside,
        );
    }

    // Extend the objective tooltip to the whole card (not just the progress
    // bar) for single-objective missions -- most dungeon/encounter cards. The
    // dungeon tooltip stays tied to that objective's portal. Multi-objective
    // cards keep their per-bar tooltips (attached above).
    let resp = if !compact && e.objectives.len() == 1 {
        let obj = &e.objectives[0];
        let counter = if obj.need > 0 {
            format!("  {}/{}", obj.have.min(obj.need), obj.need)
        } else {
            String::new()
        };
        attach_objective_tooltip(resp, sr, obj, format!("{}{}", obj.label, counter))
    } else {
        resp
    };

    // Move cursor when hovering the card background signals it can be dragged
    // to reorder. Child widgets (Hide, badges) keep their own cursors.
    if resp.hovered() {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Move);
    }

    ui.add_space(CARD_GAP);
    resp
}

impl Panel for MissionsPanel {
    fn show(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext) -> Vec<AppAction> {
        let view = ctx.mission_view;

        // Definitions not loaded: don't render live progress diffs as half-baked
        // placeholder rows. Prompt the user to Refresh instead.
        if !view.defs_loaded {
            empty_state_warning(
                ui,
                "Mission data not loaded",
                Some("Press Refresh (top-right) while logged in to load your missions."),
            );
            return Vec::new();
        }

        if view.entries.is_empty() {
            empty_state(
                ui,
                "No mission data yet",
                Some("Press Refresh (top-right) while logged in to load your missions."),
            );
            return Vec::new();
        }

        let shadcn = ctx.shadcn;
        let card_fill = shadcn.secondary_header_fill();

        // Resolve the live character once (skin/dyes + seasonal/crucible flags)
        // so each card can show its sprite and whether it qualifies. Owned/Copy
        // so we don't hold an `account_data` borrow across the render loop.
        let current_char = ctx.live_char_id.and_then(|id| {
            ctx.account_data.find_character(id).map(|c| CurrentChar {
                skin_id: if c.skin > 0 {
                    c.skin
                } else {
                    c.class_id as i32
                },
                class_id: c.class_id as i32,
                tex1: c.tex1,
                tex2: c.tex2,
                seasonal: c.seasonal,
                crucible_active: c.crucible_active,
                equipped_slots: equipped_slot_types(c),
            })
        });

        // Header band: battlepass title + label filter chips + hide-claimed
        // toggle, in a darker full-bleed band with a divider like other tabs.
        shadcn.header_band(ui, shadcn.secondary_header_fill(), |ui| {
            shadcn.band_row(ui, |ui| {
                let title = if view.season_name.is_empty() {
                    "Missions".to_string()
                } else {
                    view.season_name.clone()
                };
                ui.heading(title).hover_tip("Current battlepass");

                ui.add_space(12.0);
                let present: HashSet<u8> =
                    view.entries.iter().map(|e| e.category.order()).collect();
                for cat in CATEGORIES {
                    if !present.contains(&cat.order()) {
                        continue;
                    }
                    let on = !self.disabled_labels.contains(&cat.order());
                    let resp = filter_badge(ui, cat, on);
                    let clicked = resp.clicked();
                    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
                        .hover_tip(format!("Click to show/hide {} missions", cat.label()));
                    if clicked {
                        if on {
                            self.disabled_labels.insert(cat.order());
                        } else {
                            self.disabled_labels.remove(&cat.order());
                        }
                    }
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(10.0);

                // Collapse/expand every status section at once.
                let any_expanded =
                    (0u8..5).any(|i| !self.collapsed.contains(&i)) || self.hidden_expanded;
                let label = if any_expanded {
                    "⏶ Collapse all"
                } else {
                    "⏷ Expand all"
                };
                if ui
                    .button(label)
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .hover_tip("Collapse or expand all mission sections")
                    .clicked()
                {
                    if any_expanded {
                        self.collapsed = (0u8..5).collect();
                        self.hidden_expanded = false;
                    } else {
                        self.collapsed.clear();
                        self.hidden_expanded = true;
                    }
                    self.dirty = true;
                }

                ui.add_space(8.0);
                if ui
                    .checkbox(&mut self.hide_claimed, "Hide claimed")
                    .hover_tip("Hides already claimed missions if they are non-repeatable")
                    .changed()
                {
                    self.dirty = true;
                }

                ui.add_space(8.0);
                if ui
                    .checkbox(&mut self.hide_name_icon, "Hide mission name & icon")
                    .hover_tip(
                        "Hides the mission icon and name columns to save space on smaller screens",
                    )
                    .changed()
                {
                    self.dirty = true;
                }
            });
        });

        let now = now_unix();

        // Drive the auto-jump even when the cooldown card isn't rendered (section
        // collapsed, category filtered, or mission hidden): schedule a repaint at
        // the soonest upcoming reset boundary, capped at 30s so countdown labels
        // still tick.
        if let Some(min_rem) = view
            .entries
            .iter()
            .filter(|e| e.repeatable && e.state == MissionState::Claimed)
            .filter_map(|e| e.reset_at)
            .map(|r| r - now)
            .filter(|&s| s > 0)
            .min()
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs(min_rem.clamp(1, 30) as u64));
        }

        // Build the visible list. A user-hidden mission still surfaces if it
        // becomes claimable (so a reward is never missed); it returns to Hidden
        // once claimed.
        let entries: Vec<&MissionEntryView> = view
            .entries
            .iter()
            .filter(|e| !self.hidden.contains(&e.uid()) || is_claimable(e))
            .filter(|e| !self.disabled_labels.contains(&e.category.order()))
            .filter(|e| !(self.hide_claimed && e.state == MissionState::Claimed && !e.repeatable))
            .collect();

        // When missions from more than one battlepass tree are visible, each
        // card shows its tree name so merged trees stay distinguishable.
        let multi_tree = view
            .entries
            .iter()
            .map(|e| e.tree_id)
            .collect::<HashSet<i32>>()
            .len()
            > 1;

        // Mission column width = the widest visible name (clamped), so names are
        // never clipped and don't spill into the objective column. Skipped when
        // the name/icon columns are hidden.
        let name_w = if self.hide_name_icon {
            0.0
        } else {
            let mut maxw = 0.0_f32;
            for e in &entries {
                let g = ui.painter().layout_no_wrap(
                    e.name.clone(),
                    FontId::proportional(NAME_SIZE),
                    Color32::WHITE,
                );
                maxw = maxw.max(g.size().x);
            }
            (maxw + 6.0).clamp(W_NAME_MIN, W_NAME_MAX)
        };

        // Column headers, pinned above the scroll area so they stay visible.
        // `reward_w` is computed here and reused inside so columns line up.
        ui.add_space(8.0);
        let reward_w =
            (ui.available_width() - PAD_X * 2.0 - fixed_span(name_w, self.hide_name_icon))
                .max(REWARD_MIN);
        header_row(ui, name_w, reward_w, self.hide_name_icon);
        shadcn.full_width_separator(ui);
        ui.add_space(6.0);

        let mut to_hide: Vec<i64> = Vec::new();
        let mut to_unhide: Vec<i64> = Vec::new();
        let mut toggled_cat: Option<u8> = None;
        let mut section_toggles: Vec<u8> = Vec::new();
        let mut toggle_track: Option<i64> = None;

        // While dragging, disable the ScrollArea's drag-to-scroll (it would
        // auto-pan from the drag gesture) and drive the wheel manually below.
        let is_dragging = self.dragging.is_some();
        let scroll_source = if is_dragging {
            egui::containers::scroll_area::ScrollSource {
                drag: false,
                ..egui::containers::scroll_area::ScrollSource::ALL
            }
        } else {
            egui::containers::scroll_area::ScrollSource::ALL
        };

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .scroll_source(scroll_source)
            .show(ui, |ui| {
                // Labels here must not be selectable; otherwise dragging a card
                // to reorder highlights its text instead.
                ui.style_mut().interaction.selectable_labels = false;
                if is_dragging {
                    let wheel = ui.input(|i| i.raw_scroll_delta.y);
                    if wheel != 0.0 {
                        ui.scroll_with_delta(egui::vec2(0.0, wheel));
                    }
                }

                // Group missions into collapsible status sections.
                let mut buckets: [Vec<&MissionEntryView>; 5] =
                    [Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()];
                for &e in &entries {
                    let idx = if e.state == MissionState::Locked {
                        4
                    } else if e.state == MissionState::Claimed {
                        if e.repeatable {
                            2
                        } else {
                            3
                        }
                    } else if is_claimable(e) {
                        1
                    } else {
                        0
                    };
                    buckets[idx].push(e);
                }
                // Within a section: user drag order first, then a per-section
                // default. Most sections default to most-progressed, but "On
                // cooldown" missions are all 100% complete, so they instead
                // default to soonest-reset-first (least time left at the top).
                const COOLDOWN_BUCKET: usize = 2;
                for (idx, bucket) in buckets.iter_mut().enumerate() {
                    let cooldown_section = idx == COOLDOWN_BUCKET;
                    bucket.sort_by(|a, b| {
                        let default = if cooldown_section {
                            // Unknown reset time sorts last.
                            let ra = a.reset_at.unwrap_or(i64::MAX);
                            let rb = b.reset_at.unwrap_or(i64::MAX);
                            ra.cmp(&rb)
                        } else {
                            completion_fraction(b)
                                .partial_cmp(&completion_fraction(a))
                                .unwrap_or(std::cmp::Ordering::Equal)
                        };
                        self.order_pos(a.uid())
                            .cmp(&self.order_pos(b.uid()))
                            .then(default)
                            .then(a.uid().cmp(&b.uid()))
                    });
                }
                let titles = [
                    "In progress",
                    "Claimable",
                    "On cooldown",
                    "Claimed",
                    "Locked",
                ];
                let colors = [
                    Color32::from_rgb(74, 149, 199),
                    Color32::from_rgb(235, 205, 70),
                    Color32::from_gray(150),
                    Color32::from_rgb(110, 190, 90),
                    Color32::from_gray(120),
                ];
                for (i, bucket) in buckets.iter().enumerate() {
                    if bucket.is_empty() {
                        continue;
                    }
                    let expanded = !self.collapsed.contains(&(i as u8));
                    if section_header(ui, expanded, titles[i], colors[i], bucket.len()) {
                        section_toggles.push(i as u8);
                    }
                    if !expanded {
                        ui.add_space(6.0);
                        continue;
                    }
                    ui.add_space(2.0);
                    let mut rects: Vec<(i64, egui::Rect)> = Vec::with_capacity(bucket.len());
                    for &e in bucket {
                        let resp = mission_card(
                            ui,
                            &mut *ctx.sprite_renderer,
                            e,
                            card_fill,
                            name_w,
                            reward_w,
                            self.dragging == Some(e.uid()),
                            self.hide_name_icon,
                            current_char,
                            &mut to_hide,
                            &mut to_unhide,
                            false,
                            &mut toggled_cat,
                            self.taskbar.enabled && i == 0,
                            self.taskbar.mission_tracked(e.uid()),
                            &mut toggle_track,
                            multi_tree,
                        );
                        rects.push((e.uid(), resp.rect));
                        if resp.drag_started() {
                            self.dragging = Some(e.uid());
                        }
                    }

                    // Drag-reorder within this section.
                    if let Some(drag_id) = self.dragging {
                        if rects.iter().any(|(id, _)| *id == drag_id) {
                            if let Some(ptr) = ui.ctx().pointer_interact_pos() {
                                let di = rects
                                    .iter()
                                    .position(|(_, r)| ptr.y < r.center().y)
                                    .unwrap_or(rects.len());
                                // Indicator line at the drop boundary.
                                let y = if di < rects.len() {
                                    rects[di].1.top()
                                } else {
                                    rects[rects.len() - 1].1.bottom()
                                } - CARD_GAP * 0.5;
                                let xr = rects[0].1.x_range();
                                ui.painter().hline(
                                    xr,
                                    y,
                                    Stroke::new(2.5_f32, Color32::from_rgb(235, 205, 70)),
                                );
                                if ui.input(|i| i.pointer.any_released()) {
                                    let ids: Vec<i64> = rects.iter().map(|(id, _)| *id).collect();
                                    let old = ids.iter().position(|&x| x == drag_id).unwrap_or(0);
                                    let mut new_ids: Vec<i64> =
                                        ids.iter().copied().filter(|&x| x != drag_id).collect();
                                    let ins = if old < di { di - 1 } else { di }.min(new_ids.len());
                                    new_ids.insert(ins, drag_id);
                                    if new_ids != ids {
                                        self.set_section_order(&new_ids);
                                    }
                                }
                            }
                        }
                    }
                    ui.add_space(6.0);
                }

                // "Hidden by user" section: same collapsible style, always last.
                // A claimable mission is never here (it surfaces in Claimable).
                let hidden_entries: Vec<&MissionEntryView> = view
                    .entries
                    .iter()
                    .filter(|e| self.hidden.contains(&e.uid()) && !is_claimable(e))
                    .collect();
                if !hidden_entries.is_empty() {
                    ui.add_space(4.0);
                    if section_header(
                        ui,
                        self.hidden_expanded,
                        "Hidden by user",
                        Color32::from_gray(150),
                        hidden_entries.len(),
                    ) {
                        self.hidden_expanded = !self.hidden_expanded;
                    }
                    if self.hidden_expanded {
                        ui.add_space(2.0);
                        for e in hidden_entries {
                            mission_card(
                                ui,
                                &mut *ctx.sprite_renderer,
                                e,
                                card_fill,
                                name_w,
                                reward_w,
                                false,
                                self.hide_name_icon,
                                current_char,
                                &mut to_hide,
                                &mut to_unhide,
                                true,
                                &mut toggled_cat,
                                false,
                                false,
                                &mut toggle_track,
                                multi_tree,
                            );
                        }
                    }
                }
            });

        // Drag ended (released anywhere): clear transient drag state.
        if self.dragging.is_some() && ui.input(|i| i.pointer.any_released()) {
            self.dragging = None;
            self.drop_index = None;
        }

        for s in section_toggles {
            if !self.collapsed.remove(&s) {
                self.collapsed.insert(s);
            }
            self.dirty = true;
        }

        let mut taskbar_changed = false;
        for id in to_hide {
            self.hidden.insert(id);
            self.dirty = true;
            // A hidden mission stops being tracked in the Taskbar so it no
            // longer shows there and its compass reads off.
            if self.taskbar.mission_tracked(id) {
                self.taskbar.set_mission_tracked(id, false);
                taskbar_changed = true;
            }
        }
        for id in to_unhide {
            self.hidden.remove(&id);
            self.dirty = true;
        }
        if let Some(order) = toggled_cat {
            if !self.disabled_labels.remove(&order) {
                self.disabled_labels.insert(order);
            }
        }

        let mut actions: Vec<AppAction> = Vec::new();
        if let Some(id) = toggle_track {
            let now = self.taskbar.mission_tracked(id);
            self.taskbar.set_mission_tracked(id, !now);
            taskbar_changed = true;
        }
        if taskbar_changed {
            actions.push(AppAction::SaveTaskbarSettings(self.taskbar.clone()));
        }
        if self.dirty {
            self.dirty = false;
            actions.push(AppAction::SaveMissionsSettings(self.to_settings()));
        }
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(seasonal: bool, crucible_active: bool) -> CurrentChar {
        CurrentChar {
            skin_id: 0,
            class_id: 0,
            tex1: 0,
            tex2: 0,
            seasonal,
            crucible_active,
            equipped_slots: [0; 4],
        }
    }

    fn worn(slot_type: i32) -> WornReqView {
        WornReqView {
            slot_type,
            display: String::new(),
        }
    }

    #[test]
    fn regular_mask_qualifies_everyone() {
        assert_eq!(mission_qualifies(255, &[], &ch(false, false)), Some(true));
        assert_eq!(mission_qualifies(255, &[], &ch(true, true)), Some(true));
    }

    #[test]
    fn seasonal_mask_requires_seasonal_char() {
        assert_eq!(mission_qualifies(2, &[], &ch(true, false)), Some(true));
        assert_eq!(mission_qualifies(2, &[], &ch(false, false)), Some(false));
    }

    #[test]
    fn crucible_mask_requires_crucible_char() {
        assert_eq!(mission_qualifies(4, &[], &ch(false, true)), Some(true));
        assert_eq!(mission_qualifies(4, &[], &ch(false, false)), Some(false));
    }

    #[test]
    fn seasonal_and_crucible_char_matches_both_masks() {
        let c = ch(true, true);
        assert_eq!(mission_qualifies(2, &[], &c), Some(true));
        assert_eq!(mission_qualifies(4, &[], &c), Some(true));
    }

    #[test]
    fn unknown_masks_yield_no_indicator() {
        assert_eq!(mission_qualifies(0, &[], &ch(true, true)), None);
        assert_eq!(mission_qualifies(1, &[], &ch(true, true)), None);
        assert_eq!(mission_qualifies(6, &[], &ch(true, true)), None);
        assert_eq!(mission_qualifies(258, &[], &ch(true, true)), None);
        assert_eq!(mission_qualifies(-1, &[], &ch(true, true)), None);
    }

    #[test]
    fn worn_restriction_requires_matching_equipped_slot() {
        // Orb (21) required. Character with an Orb equipped qualifies; one with
        // only other gear (or no readable equipment) stays dimmed.
        let mut with_orb = ch(true, false);
        with_orb.equipped_slots = [1, 21, 7, 9];
        let mut without_orb = ch(true, false);
        without_orb.equipped_slots = [1, 8, 7, 9];
        let unknown = ch(true, false);

        assert_eq!(mission_qualifies(2, &[worn(21)], &with_orb), Some(true));
        assert_eq!(mission_qualifies(2, &[worn(21)], &without_orb), Some(false));
        assert_eq!(mission_qualifies(2, &[worn(21)], &unknown), Some(false));
        // Restriction is ANDed with the participant mask: a non-seasonal char
        // still fails the seasonal gate even with the right item.
        let mut non_seasonal = ch(false, false);
        non_seasonal.equipped_slots = [1, 21, 7, 9];
        assert_eq!(
            mission_qualifies(2, &[worn(21)], &non_seasonal),
            Some(false)
        );
    }

    #[test]
    fn unknown_character_defaults_to_least_privileged() {
        // No live character yet: regular missions stay eligible, but seasonal,
        // crucible, and worn-restricted ones start dimmed until a qualifying
        // character is confirmed. Unknown masks still stay bright.
        assert!(mission_eligible(None, 255, &[]));
        assert!(!mission_eligible(None, 2, &[]));
        assert!(!mission_eligible(None, 4, &[]));
        assert!(!mission_eligible(None, 255, &[worn(21)]));
        assert!(mission_eligible(None, 0, &[]));
    }
}

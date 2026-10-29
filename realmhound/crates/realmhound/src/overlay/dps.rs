//! Live DPS overlay body.
//!
//! Shows one boss fight of the map instance the player is in, with a tab per
//! boss (phases included, since each phase is its own fight). The list is
//! produced by
//! [`realmhound_core::combat::CombatTracker::live_snapshots`]. In a dungeon it
//! only empties on a zone change, so hitting a second boss adds a tab instead
//! of wiping the numbers that were on screen; in the realm, where bosses never
//! run out, it is narrowed to the encounter the player is currently on.
//!
//! Each tab is a mid-fight projection, so the local player's own damage can
//! still be an estimate (see [`DamageProvenance`]) -- their row is tinted to say
//! so rather than quietly presenting a guess as fact.

use std::sync::Arc;

use eframe::egui::{self, Color32, RichText};
use realmhound_core::combat::{DamageProvenance, FightParticipant, LiveFightSnapshot};

use crate::rendering::SpriteRenderer;

use super::{
    overlay_tooltip, overlay_tooltip_with, row_text, scrollbar_hit, thousands, HitRegions,
    OverlayContent,
};

/// Side of the per-row class sprite.
const ICON: f32 = 16.0;
/// Facing used for every character sprite, matching the Combat History rows.
const SPRITE_DIRECTION: i32 = 6;
/// Fill behind a row, scaled by that participant's share of total damage.
const SHARE_FILL: Color32 = Color32::from_rgb(60, 90, 150);
/// Row tint for the local player.
const LOCAL_COLOR: Color32 = Color32::from_rgb(120, 215, 130);
/// Row tint for the local player when their own figure is still an estimate or
/// a lower bound. Deliberately never applied to anyone else: an amber remote row
/// reads as a warning about *that player*, which it is not.
const UNCERTAIN_COLOR: Color32 = Color32::from_rgb(230, 190, 110);
const NORMAL_COLOR: Color32 = Color32::from_rgb(225, 225, 230);
/// Tint of the selected tab in the strip, and of the rest.
const TAB_ACTIVE: Color32 = Color32::from_rgb(200, 215, 240);
const TAB_IDLE: Color32 = Color32::from_gray(120);
/// Underline under a tab whose fight is still running.
const TAB_LIVE: Color32 = Color32::from_rgb(180, 70, 70);
/// Height reserved for the tab strip.
const TAB_HEIGHT: f32 = 18.0;
/// Longest boss name rendered on a tab before it is ellipsized.
const TAB_LABEL_CHARS: usize = 12;
/// How far ahead a boss's last local hit must be before auto-follow moves to it.
/// Without the margin, hitting two bosses in alternation swaps the tab on every
/// packet batch.
const SWITCH_MARGIN_MS: i64 = 1_500;

#[derive(Default)]
pub(super) struct DpsOverlay {
    /// Boss whose tab the user clicked, which overrides auto-follow until they
    /// click it again or change zone.
    pinned: Option<i32>,
    /// Boss auto-follow last settled on, kept so the hysteresis below has
    /// something to compare against.
    followed: Option<i32>,
    /// Boss ids of the last instance rendered. A zone change swaps the whole set
    /// out, which is the one moment the overlay resets its selection.
    last_ids: Vec<i32>,
}

/// Borrows everything one frame of the DPS overlay needs.
pub(super) struct DpsContent<'a> {
    pub overlay: &'a mut DpsOverlay,
    pub fights: &'a [Arc<LiveFightSnapshot>],
    pub follow: &'a mut bool,
    pub max_rows: usize,
}

impl OverlayContent for DpsContent<'_> {
    fn title_extras(&mut self, ui: &mut egui::Ui) {
        let color = if *self.follow {
            LOCAL_COLOR
        } else {
            Color32::from_gray(110)
        };
        let response = ui.add(
            egui::Button::new(RichText::new("me").size(11.0).color(color))
                .frame(false)
                .min_size(egui::vec2(20.0, 0.0)),
        );
        overlay_tooltip(ui, &response, "Pin my own row above the list");
        if response.clicked() {
            *self.follow = !*self.follow;
        }
    }

    fn body(&mut self, ui: &mut egui::Ui, sprites: &mut SpriteRenderer, hits: &mut HitRegions) {
        self.overlay
            .body(ui, sprites, self.fights, self.max_rows, *self.follow, hits);
    }
}

impl DpsOverlay {
    fn body(
        &mut self,
        ui: &mut egui::Ui,
        sprites: &mut SpriteRenderer,
        fights: &[Arc<LiveFightSnapshot>],
        max_rows: usize,
        follow: bool,
        hits: &mut HitRegions,
    ) {
        let fight: Option<&LiveFightSnapshot> = if fights.is_empty() {
            self.last_ids.clear();
            self.pinned = None;
            self.followed = None;
            None
        } else {
            self.sync_instance(fights);
            Some(
                self.pinned
                    .or(self.followed)
                    .and_then(|id| fights.iter().find(|f| f.boss_object_id == id))
                    .map(|f| &**f)
                    .unwrap_or(&fights[0]),
            )
        };

        let total = fight.map_or(1, |f| f.total_damage().max(1));
        let mut pinned = None;
        if let Some(fight) = fight {
            if fights.len() > 1 {
                self.tab_strip(ui, fights, fight.boss_object_id, hits);
                ui.add_space(2.0);
            }

            header(ui, fight);
            ui.add_space(2.0);

            pinned = follow
                .then(|| fight.participants.iter().position(|p| p.is_local))
                .flatten();
            if let Some(index) = pinned {
                row(
                    ui,
                    sprites,
                    &fight.participants[index],
                    index + 1,
                    total,
                    hits,
                );
                ui.add_space(2.0);
            }
        }

        let output = egui::ScrollArea::vertical()
            .id_salt("dps_rows")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let Some(fight) = fight else {
                    ui.vertical_centered(|ui| {
                        ui.add_space(8.0);
                        ui.label(row_text("No fight in progress", Color32::from_gray(120)));
                    });
                    return;
                };
                let rows: &[FightParticipant] = if max_rows == 0 {
                    &fight.participants
                } else {
                    let end = max_rows.min(fight.participants.len());
                    &fight.participants[..end]
                };
                for (index, participant) in rows.iter().enumerate() {
                    if pinned == Some(index) {
                        continue;
                    }
                    row(ui, sprites, participant, index + 1, total, hits);
                }
                if max_rows > 0 && fight.participants.len() > rows.len() {
                    let hidden = fight.participants.len() - rows.len();
                    ui.label(row_text(format!("+{hidden} more"), Color32::from_gray(110)));
                }
            });
        if let Some(bar) = scrollbar_hit(&output) {
            hits.add(bar);
        }
    }

    /// Point the overlay at the boss the player is currently attacking, and
    /// reset the selection when they have changed zone.
    ///
    /// A new instance shares no boss ids with the old one; within an instance
    /// fights are only ever appended, so a prefix match means "same zone, more
    /// bosses" and a pinned tab survives.
    ///
    /// Auto-follow uses the local-hit clock, not general fight activity: a boss
    /// merely standing in view refreshes the latter, which is what used to make
    /// the display flip between rows. It also requires the newcomer to be ahead
    /// by [`SWITCH_MARGIN_MS`], so alternating between two bosses settles on one
    /// instead of swapping every packet batch.
    fn sync_instance(&mut self, fights: &[Arc<LiveFightSnapshot>]) {
        let ids: Vec<i32> = fights.iter().map(|f| f.boss_object_id).collect();
        if self.last_ids.is_empty() || !ids.starts_with(&self.last_ids) {
            self.pinned = None;
            self.followed = None;
        }
        self.last_ids = ids;

        let newest = fights
            .iter()
            .filter(|f| f.last_local_hit_at > 0)
            .max_by_key(|f| f.last_local_hit_at);
        if let Some(target) = newest {
            let switch = match self.followed {
                // Nothing to displace yet: the first boss hit wins outright.
                None => true,
                Some(id) if id == target.boss_object_id => false,
                Some(id) => {
                    let incumbent = fights
                        .iter()
                        .find(|f| f.boss_object_id == id)
                        .map(|f| f.last_local_hit_at)
                        .unwrap_or(0);
                    target.last_local_hit_at >= incumbent + SWITCH_MARGIN_MS
                }
            };
            if switch {
                self.followed = Some(target.boss_object_id);
            }
        }

        // A selected tab can still disappear: the retention cap evicts the
        // oldest fight of a very long run. Fall back rather than blank.
        let gone = |id: &i32| !fights.iter().any(|f| f.boss_object_id == *id);
        if self.followed.as_ref().is_some_and(gone) {
            self.followed = None;
        }
        if self.pinned.as_ref().is_some_and(gone) {
            self.pinned = None;
        }
    }

    /// Horizontal, scrollable strip with one entry per boss.
    fn tab_strip(
        &mut self,
        ui: &mut egui::Ui,
        fights: &[Arc<LiveFightSnapshot>],
        selected: i32,
        hits: &mut HitRegions,
    ) {
        let top = ui.cursor().top();
        egui::ScrollArea::horizontal()
            .id_salt("dps_tabs")
            .auto_shrink([false, true])
            .max_height(TAB_HEIGHT)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let pinned = self.pinned.is_some();

                    for fight in fights {
                        let boss = fight.boss_object_id;
                        let active = boss == selected;
                        let label = truncate(&fight.boss_name, TAB_LABEL_CHARS);
                        let response = tab_button(ui, &label, active, !fight.ended);
                        if response.clicked() {
                            self.toggle_pin(boss);
                        }
                        overlay_tooltip_with(ui, &response, || {
                            pin_hint(
                                &format!(
                                    "{}\n{} damage over {}",
                                    fight.boss_name,
                                    thousands(fight.total_damage()),
                                    elapsed(fight.elapsed_ms),
                                ),
                                pinned && active,
                            )
                        });
                    }
                });
            });

        // The whole strip, bar included: a locked overlay has to stay able to
        // switch boss and to scroll to a tab that is off the right edge.
        let laid_out = ui.min_rect();
        hits.add(egui::Rect::from_min_max(
            egui::pos2(laid_out.left(), top),
            egui::pos2(laid_out.right(), laid_out.bottom()),
        ));
    }

    /// Clicking the tab auto-follow is already on resumes following; clicking
    /// any other pins it, so the user can park on a boss while fighting another.
    fn toggle_pin(&mut self, boss: i32) {
        self.pinned = match self.pinned {
            Some(current) if current == boss => None,
            _ => Some(boss),
        };
    }
}

/// Append the pin state to a tab's hover text.
fn pin_hint(body: &str, pinned: bool) -> String {
    if pinned {
        format!("{body}\n\nPinned. Click again to follow the boss you are attacking.")
    } else {
        format!("{body}\n\nClick to pin; the overlay otherwise follows the boss you are attacking.")
    }
}

/// One tab: a flat label, underlined in red while its fight is still running.
fn tab_button(ui: &mut egui::Ui, label: &str, selected: bool, live: bool) -> egui::Response {
    let color = if selected { TAB_ACTIVE } else { TAB_IDLE };
    let text = RichText::new(label).size(11.0).color(color);
    let text = if selected { text.strong() } else { text };
    let response = ui.add(
        egui::Button::new(text)
            .frame(false)
            .min_size(egui::vec2(0.0, TAB_HEIGHT)),
    );
    if live {
        let rect = response.rect;
        ui.painter().line_segment(
            [
                egui::pos2(rect.left(), rect.bottom() - 1.0),
                egui::pos2(rect.right(), rect.bottom() - 1.0),
            ],
            egui::Stroke::new(1.5_f32, TAB_LIVE),
        );
    }
    response
}

/// Shorten a boss name to fit a tab, keeping a trailing ellipsis.
fn truncate(name: &str, max_chars: usize) -> String {
    if name.chars().count() <= max_chars {
        return name.to_string();
    }
    let kept: String = name.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}\u{2026}", kept.trim_end())
}

/// Boss name, elapsed timer and HP bar.
fn header(ui: &mut egui::Ui, fight: &LiveFightSnapshot) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(fight.boss_name.clone())
                .size(12.0)
                .strong()
                .color(Color32::from_gray(235)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(row_text(elapsed(fight.elapsed_ms), Color32::from_gray(170)));
        });
    });

    // HP is only drawn once the server has actually reported a pool; a boss we
    // have never seen a stat for would otherwise render as a full-or-empty bar
    // that means nothing.
    if fight.boss_max_hp <= 0 || fight.boss_hp < 0 {
        return;
    }
    let fraction = (fight.boss_hp as f32 / fight.boss_max_hp as f32).clamp(0.0, 1.0);
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 6.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, Color32::from_rgb(40, 40, 46));
    let mut filled = rect;
    filled.set_width(rect.width() * fraction);
    painter.rect_filled(filled, 2.0, Color32::from_rgb(180, 70, 70));
}

/// One participant row, with a share-of-damage bar painted behind it.
fn row(
    ui: &mut egui::Ui,
    sprites: &mut SpriteRenderer,
    participant: &FightParticipant,
    rank: usize,
    total: i64,
    hits: &mut HitRegions,
) {
    let share = (participant.damage as f32 / total as f32).clamp(0.0, 1.0);
    let color = row_color(participant);

    ui.horizontal(|ui| {
        ui.set_height(ICON);
        let full = ui.available_rect_before_wrap();
        let mut bar = full;
        bar.set_width(full.width() * share);
        ui.painter()
            .rect_filled(bar, 2.0, SHARE_FILL.gamma_multiply(0.35));

        ui.label(row_text(format!("{rank}."), Color32::from_gray(130)));

        let (rect, _) = ui.allocate_exact_size(egui::vec2(ICON, ICON), egui::Sense::hover());
        let icon_id = if participant.skin_id > 0 {
            participant.skin_id
        } else {
            participant.object_type
        };
        if icon_id > 0 {
            sprites.draw_dyed_outlined_character_sprite(
                ui,
                icon_id,
                rect,
                SPRITE_DIRECTION,
                participant.tex1,
                participant.tex2,
            );
        }

        let name = ui.add(
            egui::Label::new(row_text(&participant.name, color))
                .sense(egui::Sense::click())
                .selectable(false),
        );
        // The name is the only thing in a row that answers a click, so it is the
        // only part of the body that may interrupt click-through. Clipped to the
        // scroll viewport so a row scrolled out of sight does not leave a live
        // patch behind.
        hits.add(name.rect.intersect(ui.clip_rect()));
        overlay_tooltip_with(ui, &name, || {
            format!("Click to copy\n{}", callout(participant, rank, share))
        });
        if name.clicked() {
            ui.ctx().copy_text(callout(participant, rank, share));
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(row_text(
                format!("{:>3.0}%", share * 100.0),
                Color32::from_gray(150),
            ));
            ui.label(row_text(thousands(participant.damage), color));
        });
    });
}

/// Green for the local player, amber when their own figure is not exact.
///
/// Uncertainty is only ever signalled on the local row. `PartiallyObserved`
/// applies to the whole roster when the player joined a fight late, so tinting
/// every row on it turned the entire overlay amber.
fn row_color(participant: &FightParticipant) -> Color32 {
    if !participant.is_local {
        return NORMAL_COLOR;
    }
    if is_uncertain(participant.provenance) {
        UNCERTAIN_COLOR
    } else {
        LOCAL_COLOR
    }
}

/// Whether this figure is an estimate, a lower bound, or not yet resolvable.
fn is_uncertain(provenance: DamageProvenance) -> bool {
    matches!(
        provenance,
        DamageProvenance::PartiallyObserved
            | DamageProvenance::SelfPartial
            | DamageProvenance::SelfPending
            | DamageProvenance::SelfEstimated
            | DamageProvenance::Unresolved
    )
}

/// `m:ss`, or `h:mm:ss` for the rare fight that runs past an hour.
fn elapsed(ms: i64) -> String {
    let total = (ms / 1000).max(0);
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// The one-line summary clicking a row's name puts on the clipboard, ready to
/// be pasted into chat: rank, player, damage and share of the fight.
fn callout(participant: &FightParticipant, rank: usize, share: f32) -> String {
    format!(
        "{rank}: {} {} {:.0}%",
        participant.name,
        thousands(participant.damage),
        share * 100.0
    )
}

#[cfg(test)]
mod tests {
    use super::{
        callout, elapsed, is_uncertain, row_color, truncate, Arc, DpsOverlay, NORMAL_COLOR,
    };
    use realmhound_core::combat::{
        DamageProvenance, FightParticipant, LiveFightSnapshot, ParticipantEndStatus,
    };

    fn participant(object_id: i32, name: &str, damage: i64, is_local: bool) -> FightParticipant {
        FightParticipant {
            object_id,
            object_type: 0x0300,
            skin_id: 0,
            tex1: 0,
            tex2: 0,
            name: name.to_string(),
            equipment: [0; 4],
            equipment_enchants: Default::default(),
            damage,
            hits: 1,
            is_local,
            provenance: DamageProvenance::Observed,
            end_status: ParticipantEndStatus::Present,
            pet: None,
            damage_taken: None,
            damage_taken_provenance: realmhound_core::combat::DamageTakenProvenance::Observed,
            damage_taken_hits: None,
            damage_blocked: None,
            guarded_damage: None,
            guarded_hits: None,
            summon_damage: 0,
            summon_hits: 0,
            summons: Vec::new(),
            weapons: Vec::new(),
            packet_damage: 0,
            packet_hits: 0,
        }
    }

    fn fight(
        id: i32,
        started_at: i64,
        elapsed_ms: i64,
        who: Vec<FightParticipant>,
    ) -> Arc<LiveFightSnapshot> {
        Arc::new(LiveFightSnapshot {
            boss_object_id: id,
            boss_object_type: 0x10E4,
            boss_name: format!("Boss {id}"),
            dungeon: "Test Dungeon".to_string(),
            boss_hp: -1,
            boss_max_hp: 0,
            started_at,
            last_local_hit_at: 0,
            elapsed_ms,
            participants: who,
            ended: true,
        })
    }

    /// A fight the local player last hit at `hit_at`.
    fn hit(id: i32, hit_at: i64) -> Arc<LiveFightSnapshot> {
        let mut snapshot = (*fight(id, 0, 1_000, vec![participant(1, "Me", 100, true)])).clone();
        snapshot.last_local_hit_at = hit_at;
        Arc::new(snapshot)
    }

    #[test]
    fn elapsed_rolls_over_into_hours() {
        assert_eq!(elapsed(0), "0:00");
        assert_eq!(elapsed(-500), "0:00");
        assert_eq!(elapsed(9_000), "0:09");
        assert_eq!(elapsed(83_000), "1:23");
        assert_eq!(elapsed(3_723_000), "1:02:03");
    }

    #[test]
    fn the_row_callout_carries_rank_name_damage_and_share() {
        let who = participant(1, "Nicky", 1_234_567, true);
        assert_eq!(callout(&who, 3, 0.4567), "3: Nicky 1,234,567 46%");
    }

    #[test]
    fn only_inexact_provenances_are_flagged() {
        assert!(!is_uncertain(DamageProvenance::Observed));
        assert!(!is_uncertain(DamageProvenance::SelfComputed));
        assert!(!is_uncertain(DamageProvenance::SelfReconciled));
        assert!(is_uncertain(DamageProvenance::SelfPending));
        assert!(is_uncertain(DamageProvenance::SelfEstimated));
        assert!(is_uncertain(DamageProvenance::PartiallyObserved));
        assert!(is_uncertain(DamageProvenance::Unresolved));
    }

    #[test]
    fn only_the_local_row_is_ever_tinted_amber() {
        // Joining a fight late marks every participant `PartiallyObserved`,
        // which used to turn the whole overlay yellow.
        let mut remote = participant(2, "Someone", 10, false);
        remote.provenance = DamageProvenance::PartiallyObserved;
        assert_eq!(row_color(&remote), NORMAL_COLOR);

        let mut local = participant(1, "Me", 10, true);
        local.provenance = DamageProvenance::PartiallyObserved;
        assert_ne!(row_color(&local), NORMAL_COLOR);
    }

    #[test]
    fn tab_labels_are_ellipsized_not_truncated_silently() {
        assert_eq!(truncate("Oryx", 12), "Oryx");
        assert_eq!(truncate("Oryx the Mad God 3", 12), "Oryx the Ma\u{2026}");
    }

    #[test]
    fn the_tab_follows_whichever_boss_the_player_is_hitting() {
        let mut overlay = DpsOverlay::default();
        overlay.sync_instance(&[hit(10, 1_000), hit(11, 0)]);
        assert_eq!(overlay.followed, Some(10));

        // The player turns on the second boss.
        overlay.sync_instance(&[hit(10, 1_000), hit(11, 9_000)]);
        assert_eq!(overlay.followed, Some(11));
    }

    #[test]
    fn fights_the_player_never_touched_are_still_listed() {
        // Auto-follow is a view hint, never a filter: a boss the group killed
        // while the local player was elsewhere must keep its tab and its rows.
        let fights = vec![
            hit(10, 0),
            fight(11, 0, 1_000, vec![participant(2, "Other", 400, false)]),
        ];
        let mut overlay = DpsOverlay::default();
        overlay.sync_instance(&fights);
        assert_eq!(overlay.followed, None, "nothing to follow yet");
    }

    #[test]
    fn alternating_between_two_bosses_does_not_swap_the_tab_every_batch() {
        // Both bosses are being hit continuously; without the margin the tab
        // would flip on whichever packet batch landed last.
        let mut overlay = DpsOverlay::default();
        overlay.sync_instance(&[hit(10, 10_000), hit(11, 0)]);
        assert_eq!(overlay.followed, Some(10));

        overlay.sync_instance(&[hit(10, 10_000), hit(11, 10_400)]);
        assert_eq!(overlay.followed, Some(10), "swapped on a 400ms lead");

        overlay.sync_instance(&[hit(10, 10_000), hit(11, 12_000)]);
        assert_eq!(overlay.followed, Some(11), "never committed");
    }

    #[test]
    fn a_pinned_tab_outranks_auto_follow() {
        let mut overlay = DpsOverlay::default();
        overlay.sync_instance(&[hit(10, 1_000), hit(11, 0)]);
        overlay.toggle_pin(10);

        overlay.sync_instance(&[hit(10, 1_000), hit(11, 50_000)]);
        assert_eq!(overlay.pinned, Some(10));
        assert_eq!(overlay.followed, Some(11), "auto-follow still tracks");

        // Clicking the pinned tab hands control back to auto-follow.
        overlay.toggle_pin(10);
        assert_eq!(overlay.pinned, None);
    }

    #[test]
    fn changing_zone_drops_the_pin_and_the_followed_boss() {
        let mut overlay = DpsOverlay::default();
        overlay.sync_instance(&[hit(10, 1_000)]);
        overlay.toggle_pin(10);

        // A new instance brings an unrelated boss id.
        overlay.sync_instance(&[hit(99, 0)]);
        assert_eq!(overlay.pinned, None);
        assert_eq!(overlay.followed, None);
    }

    #[test]
    fn a_new_boss_in_the_same_zone_keeps_the_pin() {
        let mut overlay = DpsOverlay::default();
        overlay.sync_instance(&[hit(10, 1_000)]);
        overlay.toggle_pin(10);

        overlay.sync_instance(&[hit(10, 1_000), hit(11, 0)]);
        assert_eq!(overlay.pinned, Some(10));
    }
}

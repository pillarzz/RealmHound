//! Party panel for displaying party members with watchlist checking.

use crate::ui_ext::HoverTooltipExt;
use eframe::egui::{self, Color32, RichText, ScrollArea, Sense, TextEdit, Ui, Vec2};
use realmhound_core::loot::class_ids;
use realmhound_core::party_cache::{PartySightingCache, PlayerSighting};
use realmhound_core::protocol::data::StatType;
use realmhound_core::settings::{PartyListLayout, PartyViewMode};
use realmhound_core::watchlist::{PlayerStatus, Watchlist};

use crate::panels::{AppAction, Panel, PanelContext};
use crate::rendering::EmbeddedIcon;
use crate::shadcn_ui::Shadcn;

// In-game sprite IDs used as party panel icons.
const SPRITE_BANLIST: i32 = 12081;
const SPRITE_REALMSCOPE: i32 = 43924;
// Char Slot Unlocker object sprite (same as the Characters tab icon), shown
// greyed with a dark overlay for party members whose class is unknown.
const SPRITE_CHAR_SLOT_UNLOCKER: i32 = 810;

/// A party member with watchlist status.
#[derive(Debug, Clone)]
pub struct PartyMember {
    pub player_id: i16,
    pub name: String,
    pub status: PlayerStatus,
    pub is_leader: bool,
    /// `None` until this player has been visible in our game world (via an
    /// Update packet), at which point class/skin/dye/guild are all populated.
    pub class_id: Option<i32>,
    /// 0 = no skin, uses default class sprite.
    pub skin_id: i32,
    pub tex1: u32,
    pub tex2: u32,
    pub guild: Option<String>,
}

impl PartyMember {
    /// Create a new party member with watchlist check.
    pub fn with_player_id(
        player_id: i16,
        name: String,
        watchlist: &Watchlist,
        is_leader: bool,
    ) -> Self {
        let status = watchlist.check_and_log(&name);
        Self {
            player_id,
            name,
            status,
            is_leader,
            class_id: None,
            skin_id: 0,
            tex1: 0,
            tex2: 0,
            guild: None,
        }
    }

    /// Create a new party member with class and skin info.
    pub fn new_with_class(name: String, watchlist: &Watchlist) -> Self {
        let status = watchlist.check_and_log(&name);
        Self {
            player_id: 0,
            name,
            status,
            is_leader: false,
            class_id: None,
            skin_id: 0,
            tex1: 0,
            tex2: 0,
            guild: None,
        }
    }

    /// Update watchlist status for this member.
    pub fn update_status(&mut self, watchlist: &Watchlist) {
        self.status = watchlist.check_player(&self.name);
    }
}

/// Party panel state and UI.
pub struct PartyPanel {
    party_name: String,
    party_id: i32,
    max_size: u8,
    members: Vec<PartyMember>,
    watchlist: Watchlist,
    is_local_leader: bool,
    editor_open: bool,
    editor_content: String,
    status_message: Option<(String, std::time::Instant)>,
    mod_tools: bool,
    view_mode: PartyViewMode,
    list_layout: PartyListLayout,
    /// Populated from Update packets as players become visible in the game world.
    seen_players: PartySightingCache,
    /// The local (main) account's player name, used to render the local player
    /// with their real skin/dyes while others use a plain class icon.
    local_player_name: Option<String>,
}

impl Default for PartyPanel {
    fn default() -> Self {
        Self {
            party_name: String::new(),
            party_id: 0,
            max_size: 0,
            members: Vec::new(),
            watchlist: Watchlist::new(),
            is_local_leader: false,
            editor_open: false,
            editor_content: String::new(),
            status_message: None,
            mod_tools: false,
            view_mode: PartyViewMode::Advanced,
            list_layout: PartyListLayout::SingleColumn,
            seen_players: PartySightingCache::new(),
            local_player_name: None,
        }
    }
}

impl PartyPanel {
    /// Create a party panel, loading the global watchlist entries from an
    /// explicit path and binding an optional per-account detection log.
    pub fn new(
        watchlist_path: std::path::PathBuf,
        detection_log: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            watchlist: Watchlist::load_from(watchlist_path, detection_log),
            ..Self::default()
        }
    }

    pub fn apply_party_settings(&mut self, settings: &realmhound_core::settings::PartySettings) {
        self.mod_tools = settings.mod_tools;
        self.view_mode = settings.view_mode;
        self.list_layout = settings.list_layout;
    }

    /// Look up a cached sighting and apply it to a member, if known.
    fn apply_known_sighting(&self, member: &mut PartyMember) {
        if let Some(sighting) = self.seen_players.get(&member.name) {
            member.class_id = Some(sighting.class_id);
            member.skin_id = sighting.skin_id;
            member.tex1 = sighting.tex1;
            member.tex2 = sighting.tex2;
            member.guild = sighting.guild.clone();
        }
    }

    fn record_sighting(&mut self, name: &str, sighting: PlayerSighting) {
        let key = name.to_lowercase();
        self.seen_players.insert(name, sighting.clone());
        if let Some(member) = self
            .members
            .iter_mut()
            .find(|m| m.name.to_lowercase() == key)
        {
            member.class_id = Some(sighting.class_id);
            member.skin_id = sighting.skin_id;
            member.tex1 = sighting.tex1;
            member.tex2 = sighting.tex2;
            member.guild = sighting.guild;
        }
    }

    fn observe_update(&mut self, update: &realmhound_core::protocol::packets::UpdatePacket) {
        for obj in &update.new_objects {
            let object_type = obj.object_type as i32;
            if !class_ids::is_player_class(object_type) {
                continue;
            }
            let name = obj
                .status
                .stats
                .iter()
                .find(|s| s.stat_type == StatType::Name)
                .and_then(|s| s.string_stat_value.clone());
            let Some(name) = name else { continue };
            let guild = obj
                .status
                .stats
                .iter()
                .find(|s| s.stat_type == StatType::GuildName)
                .and_then(|s| s.string_stat_value.clone())
                .filter(|g| !g.is_empty());
            let skin_id = obj
                .status
                .stats
                .iter()
                .find(|s| s.stat_type == StatType::SkinId)
                .map(|s| s.stat_value)
                .unwrap_or(0);
            let tex1 = obj
                .status
                .stats
                .iter()
                .find(|s| s.stat_type == StatType::Texture1)
                .map(|s| s.stat_value as u32)
                .unwrap_or(0);
            let tex2 = obj
                .status
                .stats
                .iter()
                .find(|s| s.stat_type == StatType::Texture2)
                .map(|s| s.stat_value as u32)
                .unwrap_or(0);
            self.record_sighting(
                &name,
                PlayerSighting {
                    class_id: object_type,
                    skin_id,
                    tex1,
                    tex2,
                    guild,
                },
            );
        }
    }

    /// Set the full party from an IncomingPartyMemberInfoPacket.
    /// Returns a list of banned player names for kick alert notifications.
    pub fn set_party(
        &mut self,
        party_info: &realmhound_core::protocol::IncomingPartyMemberInfoPacket,
    ) -> Vec<String> {
        self.party_id = party_info.party_id;
        self.max_size = party_info.max_size;
        self.party_name = party_info.description.clone();

        // Clear existing members and repopulate
        self.members.clear();
        let mut banned_names = Vec::new();

        for player in party_info.party_players.iter() {
            let is_leader = player.id == party_info.leader_id;
            let mut member = PartyMember::with_player_id(
                player.id,
                player.name.clone(),
                &self.watchlist,
                is_leader,
            );
            self.apply_known_sighting(&mut member);

            // Track banned members for kick alerts
            if member.status == PlayerStatus::Banned {
                banned_names.push(player.name.clone());
            }

            self.members.push(member);
        }

        // Sort: leader first, then alphabetically
        self.members
            .sort_by(|a, b| match (a.is_leader, b.is_leader) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            });

        banned_names
    }

    /// Add a single member from a PartyMemberAddedPacket.
    /// Returns the player's watchlist status (for kick alert notifications).
    pub fn add_member(
        &mut self,
        member_added: &realmhound_core::protocol::PartyMemberAddedPacket,
    ) -> PlayerStatus {
        // Check if member already exists (by name)
        if self.members.iter().any(|m| m.name == member_added.name) {
            return PlayerStatus::Normal;
        }

        let mut member = PartyMember::new_with_class(member_added.name.clone(), &self.watchlist);
        member.player_id = member_added.player_id;
        self.apply_known_sighting(&mut member);
        let status = member.status;
        self.members.push(member);

        // Re-sort: leader first, then alphabetically
        self.members
            .sort_by(|a, b| match (a.is_leader, b.is_leader) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            });

        status
    }

    /// Remove a member by name (e.g., when they leave the party).
    /// Returns true if the member was found and removed.
    pub fn remove_member_by_name(&mut self, name: &str) -> bool {
        let initial_len = self.members.len();
        self.members.retain(|m| m.name != name);
        self.members.len() < initial_len
    }

    /// Remove a member by player_id (from inbound PartyAction packet).
    /// Returns the removed member's name, if found.
    pub fn remove_member_by_id(&mut self, player_id: i16) -> Option<String> {
        if let Some(pos) = self.members.iter().position(|m| m.player_id == player_id) {
            let name = self.members.remove(pos).name;
            Some(name)
        } else {
            None
        }
    }

    /// Set whether the local player is currently the party leader.
    pub fn set_local_leader(&mut self, is_leader: bool) {
        self.is_local_leader = is_leader;
    }

    /// Set the local (main) account's player name, used to render the local
    /// player with their real skin/dyes.
    pub fn set_local_player_name(&mut self, name: Option<String>) {
        self.local_player_name = name;
    }

    /// Wipe all party state (members, identity, leader status). Used when a
    /// rejected mule/alt connection had tentatively populated the panel.
    pub fn clear(&mut self) {
        self.party_name.clear();
        self.party_id = 0;
        self.max_size = 0;
        self.members.clear();
        self.is_local_leader = false;
    }

    /// Returns whether the local player is the party leader.
    pub fn is_local_leader(&self) -> bool {
        self.is_local_leader
    }

    pub fn mod_tools(&self) -> bool {
        self.mod_tools
    }

    /// Check a join request name against the banlist. Only fires when the
    /// local player is the party leader and the banlist has entries.
    /// Returns the player name if banned, for kick alert purposes.
    fn check_join_request(&self, player_name: &str) -> Option<String> {
        if !self.is_local_leader || self.watchlist.banned_count() == 0 {
            return None;
        }
        if self.watchlist.check_player(player_name) == PlayerStatus::Banned {
            Some(player_name.to_string())
        } else {
            None
        }
    }

    /// Get the number of members in the party.
    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    pub fn banned_count(&self) -> usize {
        self.members
            .iter()
            .filter(|m| m.status == PlayerStatus::Banned)
            .count()
    }

    pub fn watched_count(&self) -> usize {
        self.members
            .iter()
            .filter(|m| m.status == PlayerStatus::Watched)
            .count()
    }

    fn set_status(&mut self, message: &str) {
        self.status_message = Some((message.to_string(), std::time::Instant::now()));
    }

    fn copy_to_clipboard(ctx: &egui::Context, text: &str) {
        ctx.copy_text(text.to_string());
    }

    /// Frameless per-row action button showing an embedded PNG icon. The icon
    /// (and its click target styling) is only drawn while the row is hovered,
    /// matching the in-game party UI. Space is always reserved so revealing the
    /// icons on hover doesn't shift the row.
    fn render_hover_embedded_button(
        sprite_renderer: &mut crate::rendering::SpriteRenderer,
        ui: &mut Ui,
        icon: EmbeddedIcon,
        size: f32,
        visible: bool,
    ) -> egui::Response {
        let padding = 3.0;
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(size + padding * 2.0, size), Sense::click());
        if visible {
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let icon_rect = egui::Rect::from_center_size(rect.center(), Vec2::splat(size));
            sprite_renderer.draw_embedded_icon(ui, icon, icon_rect);
        }
        response
    }

    fn render_sprite_icon(
        sprite_renderer: &mut crate::rendering::SpriteRenderer,
        ui: &mut Ui,
        item_id: i32,
        size: f32,
        fallback: &str,
    ) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
        if !sprite_renderer.draw_outlined_sprite_in_rect(ui, item_id, rect) {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                fallback,
                egui::FontId::proportional(size * 0.8),
                ui.visuals().text_color(),
            );
        }
        response
    }

    /// Frameless per-row action button showing an in-game sprite, only visible
    /// while the row is hovered. See [`Self::render_hover_embedded_button`].
    fn render_hover_sprite_button(
        sprite_renderer: &mut crate::rendering::SpriteRenderer,
        ui: &mut Ui,
        item_id: i32,
        size: f32,
        visible: bool,
    ) -> egui::Response {
        let padding = 3.0;
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(size + padding * 2.0, size), Sense::click());
        if visible {
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let icon_rect = egui::Rect::from_center_size(rect.center(), Vec2::splat(size));
            sprite_renderer.draw_sprite_in_rect(ui, item_id, icon_rect);
        }
        response
    }

    /// Returns (clipboard_copy, open_url) side effects to apply after layout.
    fn render_member_rows(
        ctx: &mut PanelContext,
        shadcn: &Shadcn,
        ui: &mut Ui,
        members: &[PartyMember],
        start_idx: usize,
        mod_tools: bool,
        simplified: bool,
        local_name: Option<&str>,
    ) -> (Option<String>, Option<String>) {
        const MARKER_SLOT: f32 = 16.0;
        const MARKER_ICON: f32 = 14.0;
        const ICON_GAP: f32 = 4.0;
        const MARKER_LEFT_PAD: f32 = 10.0;
        const ROW_HEIGHT: f32 = 28.0;
        const ROW_GAP: f32 = 6.0;
        const NUMBER_INDENT: f32 = MARKER_LEFT_PAD + MARKER_SLOT + ICON_GAP;

        let mut clipboard_copy: Option<String> = None;
        let mut open_url: Option<String> = None;

        egui::Grid::new(("party_members_grid", start_idx))
            .num_columns(5)
            .spacing([8.0, ROW_GAP])
            .min_row_height(ROW_HEIGHT)
            .show(ui, |ui| {
                // Column headers
                {
                    let header_top = ui.cursor().top();
                    let header_rect = egui::Rect::from_min_size(
                        egui::pos2(ui.max_rect().left(), header_top),
                        egui::vec2(ui.max_rect().width(), ROW_HEIGHT),
                    );
                    ui.painter().rect_filled(
                        header_rect,
                        2.0,
                        Color32::from_rgba_unmultiplied(255, 255, 255, 22),
                    );
                }
                ui.horizontal(|ui| {
                    ui.add_space(NUMBER_INDENT);
                    ui.label(RichText::new("#").strong().color(Color32::WHITE));
                });
                ui.label(RichText::new("Player").strong().color(Color32::WHITE));
                ui.allocate_exact_size(Vec2::new(24.0, 1.0), Sense::hover());
                if !simplified {
                    ui.label(RichText::new("Guild").strong().color(Color32::WHITE));
                } else {
                    ui.label("");
                }
                ui.label("");
                ui.end_row();

                for (i, member) in members.iter().enumerate() {
                    let idx = start_idx + i;

                    // Per-row rounded card (in-game party style, matching the
                    // Loot/Combat History cards). Banned members get a red card;
                    // hovering a row reveals its action buttons.
                    let row_top = ui.cursor().top();
                    let row_rect = egui::Rect::from_min_size(
                        egui::pos2(ui.max_rect().left(), row_top),
                        egui::vec2(ui.max_rect().width(), ROW_HEIGHT),
                    );
                    let hovered = ui.rect_contains_pointer(row_rect);
                    let card_fill = if member.status == PlayerStatus::Banned {
                        Color32::from_rgb(80, 20, 20)
                    } else {
                        shadcn.secondary_header_fill()
                    };
                    ui.painter().rect_filled(row_rect, 6.0, card_fill);
                    if hovered {
                        ui.painter().rect_stroke(
                            row_rect,
                            6.0,
                            egui::Stroke::new(1.0_f32, shadcn.border()),
                            egui::StrokeKind::Inside,
                        );
                    }

                    // Single first-column marker (leader > banned > watched),
                    // fixed size with equal margins so numbers stay aligned.
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.add_space(MARKER_LEFT_PAD);

                        // Grid's min_row_height centers every cell uniformly, so a
                        // plain fixed-size square keeps the marker aligned with the
                        // sprite/name on the same vertical guideline.
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::splat(MARKER_SLOT), Sense::hover());
                        let marker = if member.is_leader {
                            Some((EmbeddedIcon::PartyLeader, "Party lead"))
                        } else if member.status == PlayerStatus::Banned {
                            Some((EmbeddedIcon::PartyBan, "⚠️ BANNED - On your ban list"))
                        } else if member.status == PlayerStatus::Watched {
                            Some((EmbeddedIcon::PartyWatch, "👁 WATCHED - On your watch list"))
                        } else {
                            None
                        };
                        if let Some((icon, tip)) = marker {
                            let icon_rect = egui::Rect::from_center_size(
                                rect.center(),
                                Vec2::splat(MARKER_ICON),
                            );
                            ctx.sprite_renderer.draw_embedded_icon(ui, icon, icon_rect);
                            response.hover_tip(tip);
                        }

                        ui.add_space(ICON_GAP);
                        ui.label(RichText::new(format!("{}.", idx + 1)).color(Color32::WHITE));
                    });

                    // Class sprite + name kept close together in one cell.
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;

                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                        if let Some(class_id) = member.class_id {
                            let is_local =
                                local_name.is_some_and(|n| n.eq_ignore_ascii_case(&member.name));
                            if is_local {
                                // Local player: render their actual skin and dyes.
                                let sprite_id = if member.skin_id > 0 {
                                    member.skin_id
                                } else {
                                    class_id
                                };
                                ctx.sprite_renderer.draw_dyed_outlined_character_sprite(
                                    ui,
                                    sprite_id,
                                    rect,
                                    6,
                                    member.tex1,
                                    member.tex2,
                                );
                            } else {
                                // Other members: plain default class appearance.
                                ctx.sprite_renderer.draw_dyed_outlined_character_sprite(
                                    ui, class_id, rect, 6, 0, 0,
                                );
                            }
                        } else if ctx.sprite_renderer.draw_outlined_sprite_in_rect(
                            ui,
                            SPRITE_CHAR_SLOT_UNLOCKER,
                            rect,
                        ) {
                            // Class unknown: greyed slot-unlocker under a dark overlay.
                            ui.painter()
                                .rect_filled(rect, 0.0, Color32::from_black_alpha(130));
                        }

                        let name_text = RichText::new(&member.name).color(Color32::WHITE);
                        let name_text = if member.is_leader || member.status != PlayerStatus::Normal
                        {
                            name_text.strong()
                        } else {
                            name_text
                        };
                        let label = ui.label(name_text);
                        label.context_menu(|ui| {
                            if shadcn.btn(ui, "Copy /pkick command").clicked() {
                                clipboard_copy = Some(format!("/pkick {}", member.name));
                                ui.close();
                            }
                        });
                    });

                    ui.allocate_exact_size(Vec2::new(24.0, 1.0), Sense::hover());

                    if !simplified {
                        if let Some(guild) = &member.guild {
                            ui.label(
                                RichText::new(guild).color(Color32::from_rgb(0x80, 0xff, 0x00)),
                            );
                        } else {
                            ui.label("");
                        }
                    } else {
                        ui.label("");
                    }

                    ui.horizontal(|ui| {
                        if Self::render_hover_embedded_button(
                            ctx.sprite_renderer,
                            ui,
                            EmbeddedIcon::PartyMessage,
                            18.0,
                            hovered,
                        )
                        .hover_tip(format!("DM {}: copy /t command", member.name))
                        .clicked()
                        {
                            clipboard_copy = Some(format!("/t {}", member.name));
                        }

                        if mod_tools && !member.is_leader {
                            if Self::render_hover_embedded_button(
                                ctx.sprite_renderer,
                                ui,
                                EmbeddedIcon::PartyPromote,
                                18.0,
                                hovered,
                            )
                            .hover_tip(format!("Promote {}: copy /ptransfer command", member.name))
                            .clicked()
                            {
                                clipboard_copy = Some(format!("/ptransfer {}", member.name));
                            }

                            if Self::render_hover_embedded_button(
                                ctx.sprite_renderer,
                                ui,
                                EmbeddedIcon::PartyKick,
                                18.0,
                                hovered,
                            )
                            .hover_tip(format!("Kick {}: copy /pkick command", member.name))
                            .clicked()
                            {
                                clipboard_copy = Some(format!("/pkick {}", member.name));
                            }
                        }

                        if !simplified {
                            if Self::render_hover_embedded_button(
                                ctx.sprite_renderer,
                                ui,
                                EmbeddedIcon::RealmeyeLogo,
                                18.0,
                                hovered,
                            )
                            .hover_tip(format!("View {}'s RealmEye profile", member.name))
                            .clicked()
                            {
                                open_url = Some(format!(
                                    "https://www.realmeye.com/player/{}",
                                    member.name
                                ));
                            }

                            if Self::render_hover_sprite_button(
                                ctx.sprite_renderer,
                                ui,
                                SPRITE_REALMSCOPE,
                                18.0,
                                hovered,
                            )
                            .hover_tip(format!("View {}'s RealmScope profile", member.name))
                            .clicked()
                            {
                                open_url =
                                    Some(format!("https://realmscope.gg/player/{}", member.name));
                            }
                        }
                    });

                    ui.end_row();
                }
            });

        (clipboard_copy, open_url)
    }

    pub fn ui(&mut self, ui: &mut Ui, ctx: &mut PanelContext) -> Vec<AppAction> {
        let shadcn = ctx.shadcn;
        let mut actions: Vec<AppAction> = Vec::new();

        shadcn.header_band(ui, shadcn.secondary_header_fill(), |ui| {
            ui.horizontal(|ui| {
                if !self.party_name.trim().is_empty() {
                    // Nudge the name down ~2px so it sits centered in the band; the
                    // taller action icons on the right fix the band height, so an
                    // asymmetric top margin offsets only the text.
                    egui::Frame::NONE
                        .inner_margin(egui::Margin {
                            left: 0,
                            right: 0,
                            top: 4,
                            bottom: 0,
                        })
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(&self.party_name)
                                    .strong()
                                    .color(Color32::WHITE),
                            );
                        });
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // The Ban List is part of the leader moderation tools, so it is
                    // only offered when those are enabled.
                    if self.mod_tools
                        && Self::render_sprite_icon(
                            ctx.sprite_renderer,
                            ui,
                            SPRITE_BANLIST,
                            22.0,
                            "📋",
                        )
                        .hover_tip("Banlist")
                        .clicked()
                    {
                        self.editor_content = self.watchlist.to_string();
                        self.editor_open = true;
                    }

                    let (layout_label, next_layout) = match self.list_layout {
                        PartyListLayout::SingleColumn => ("1 Col", PartyListLayout::AutoTwoColumn),
                        PartyListLayout::AutoTwoColumn => ("2 Col", PartyListLayout::SingleColumn),
                    };
                    if shadcn
                        .btn(ui, format!("▤ {layout_label}"))
                        .hover_tip(
                            "Toggle member list layout: single scrolling column, or \
                         auto-split into 2 columns when the list doesn't fit.",
                        )
                        .clicked()
                    {
                        self.list_layout = next_layout;
                        actions.push(AppAction::SavePartyListLayout(next_layout));
                    }

                    ui.label(
                        RichText::new(format!("Members: {}", self.member_count()))
                            .color(Color32::WHITE),
                    );
                });
            });
        });

        // Status message (temporary)
        if let Some((msg, time)) = &self.status_message {
            if time.elapsed().as_secs() < 3 {
                ui.label(RichText::new(msg).color(Color32::GREEN));
            } else {
                self.status_message = None;
            }
        }

        // Member list
        if self.members.is_empty() {
            crate::panels::empty_state(
                ui,
                "No party members",
                Some("Join a party to see members here"),
            );
        } else {
            let simplified = self.view_mode == PartyViewMode::Simplified;
            let mod_tools = self.mod_tools;
            let local_name = self.local_player_name.as_deref();

            // Row pitch must match render_member_rows (card ROW_HEIGHT + ROW_GAP)
            // so the auto two-column split estimates the fit correctly; otherwise
            // a full 50-member party wrongly looks like it fits one column.
            const ROW_PITCH: f32 = 34.0;
            const HEADER_HEIGHT: f32 = 20.0;

            let available_height = ui.available_height();
            let capacity = (((available_height - HEADER_HEIGHT) / ROW_PITCH)
                .floor()
                .max(1.0)) as usize;
            let use_two_columns =
                self.list_layout == PartyListLayout::AutoTwoColumn && self.members.len() > capacity;

            let (clipboard_copy, open_url) = if use_two_columns {
                let split_at = self.members.len().div_ceil(2);
                let (left, right) = self.members.split_at(split_at);
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.columns(2, |columns| {
                            let (left_result, right_result) = columns.split_at_mut(1);
                            let l = Self::render_member_rows(
                                ctx,
                                shadcn,
                                &mut left_result[0],
                                left,
                                0,
                                mod_tools,
                                simplified,
                                local_name,
                            );
                            let r = Self::render_member_rows(
                                ctx,
                                shadcn,
                                &mut right_result[0],
                                right,
                                split_at,
                                mod_tools,
                                simplified,
                                local_name,
                            );
                            (l.0.or(r.0), l.1.or(r.1))
                        })
                    })
                    .inner
            } else {
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        Self::render_member_rows(
                            ctx,
                            shadcn,
                            ui,
                            &self.members,
                            0,
                            mod_tools,
                            simplified,
                            local_name,
                        )
                    })
                    .inner
            };

            if let Some(command) = clipboard_copy {
                Self::copy_to_clipboard(ui.ctx(), &command);
                self.set_status(&format!("Copied: {}", command));
            }
            if let Some(url) = open_url {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
        }

        // Ban list editor modal. Always render so the dialog can maintain its
        // internal open-state bookkeeping; it early-returns while closed. Gating
        // this on `editor_open` left stale state that swallowed the next open click.
        self.render_editor(ui, shadcn);

        actions
    }

    /// Render the ban list editor modal.
    fn render_editor(&mut self, ui: &mut Ui, shadcn: &Shadcn) {
        let mut open = self.editor_open;
        let mut close_requested = false;
        shadcn.dialog(
            ui,
            "ban_list_editor",
            &mut open,
            "📋 Ban List Editor",
            400.0,
            500.0,
            |ui| {
                ui.label("Edit your ban and watch lists below.");
                ui.label(RichText::new("Format: One name per line. Add \"On watchlist:\" separator for watch list.").small().color(Color32::GRAY));

                ui.add_space(5.0);

                // Text editor
                ScrollArea::vertical()
                    .max_height(350.0)
                    .show(ui, |ui| {
                        ui.add(
                            TextEdit::multiline(&mut self.editor_content)
                                .desired_width(f32::INFINITY)
                                .desired_rows(20)
                                .font(egui::TextStyle::Monospace),
                        );
                    });

                ui.add_space(10.0);

                // Buttons
                ui.horizontal(|ui| {
                    if shadcn.button(ui, "💾 Save").clicked() {
                        self.watchlist.replace_entries(&self.editor_content);
                        if let Err(e) = self.watchlist.save() {
                            self.set_status(&format!("Error saving: {}", e));
                        } else {
                            self.set_status("Ban list saved");
                            for member in &mut self.members {
                                member.update_status(&self.watchlist);
                            }
                        }
                        close_requested = true;
                    }

                    if shadcn.button(ui, "❌ Cancel").clicked() {
                        close_requested = true;
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(format!(
                            "Banned: {} | Watched: {}",
                            Watchlist::parse(&self.editor_content).banned_count(),
                            Watchlist::parse(&self.editor_content).watched_count()
                        ));
                    });
                });
            },
        );
        if close_requested {
            open = false;
        }
        self.editor_open = open;
    }
}

impl Panel for PartyPanel {
    fn show(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext) -> Vec<AppAction> {
        self.ui(ui, ctx)
    }

    fn handle_event(
        &mut self,
        event: &realmhound_core::GameEvent,
        _session: &realmhound_core::GameSession,
    ) -> Vec<AppAction> {
        match event {
            realmhound_core::GameEvent::PartyListReceived(ref party_info) => {
                let banned_names = self.set_party(party_info);
                banned_names
                    .into_iter()
                    .map(AppAction::PushKickAlert)
                    .collect()
            }
            realmhound_core::GameEvent::PartyMemberJoined(ref member_added) => {
                let status = self.add_member(member_added);
                if status == PlayerStatus::Banned {
                    vec![AppAction::PushKickAlert(member_added.name.clone())]
                } else {
                    vec![]
                }
            }
            realmhound_core::GameEvent::PartyJoinRequestResponse(ref response) => {
                if response.state == 1 {
                    if let Some(name) = self.check_join_request(&response.player_name) {
                        vec![AppAction::PushJoinRequestAlert(name)]
                    } else {
                        vec![]
                    }
                } else {
                    vec![]
                }
            }
            realmhound_core::GameEvent::PartyActionReceived {
                player_id,
                action_id,
            } => {
                // action 2 = Kicked, 6 = LeftParty
                if *action_id == 2 || *action_id == 6 {
                    self.remove_member_by_id(*player_id);
                }
                vec![]
            }
            realmhound_core::GameEvent::UpdateReceived(ref update, _time_ms) => {
                self.observe_update(update);
                vec![]
            }
            _ => vec![],
        }
    }
}

//! Chat panel for displaying game chat messages.

use crate::ui_ext::HoverTooltipExt;
use chrono::{DateTime, Local, NaiveDate, NaiveTime};
use eframe::egui::text::LayoutJob;
use eframe::egui::{self, Color32, RichText, ScrollArea, Sense, TextFormat, Ui};
use realmhound_core::assets::get_asset_manager;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::rendering::emotes::{self, EmoteSegment, EMOTE_PADDING, EMOTE_SPRITE_SIZE};
use crate::rendering::SpriteRenderer;

/// Background color used to highlight matched search substrings.
const SEARCH_HIGHLIGHT: Color32 = Color32::from_rgb(94, 80, 16);
/// Background color flashed on a message after jumping to it.
const JUMP_FLASH: Color32 = Color32::from_rgba_premultiplied(70, 70, 30, 160);
/// How long the jump highlight flash lasts.
const JUMP_FLASH_DURATION: Duration = Duration::from_millis(900);
/// Player-name color in the chat list (yellow, mirroring in-game chat).
const NAME_YELLOW: Color32 = Color32::from_rgb(255, 220, 40);
/// Outline color drawn behind player names for the in-game "outlined text" look.
const NAME_OUTLINE: Color32 = Color32::from_rgb(20, 20, 20);

use crate::panels::{AppAction, Panel, PanelContext};

/// Type of chat message for color-coding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatType {
    /// Normal public chat (white)
    Normal,
    /// Guild chat (green)
    Guild,
    /// Party chat (purple)
    Party,
    /// Private message / whisper (light blue)
    Whisper,
    /// Server/system announcement (yellow)
    Announcement,
    /// Enemy/NPC chat (orange)
    Enemy,
}

impl ChatType {
    /// Get the color for this chat type.
    pub fn color(&self) -> Color32 {
        match self {
            ChatType::Normal => Color32::WHITE,
            ChatType::Guild => Color32::from_rgb(100, 255, 100), // Green
            ChatType::Party => Color32::from_rgb(200, 100, 255), // Purple
            ChatType::Whisper => Color32::from_rgb(100, 200, 255), // Light blue
            ChatType::Announcement => Color32::from_rgb(255, 255, 100), // Yellow
            ChatType::Enemy => Color32::from_rgb(255, 180, 100), // Orange
        }
    }
}

/// Which fields a chat search query is matched against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchScope {
    /// Match the player name(s) and the message text.
    #[default]
    All,
    /// Match the player name(s) only.
    Name,
    /// Match the message text only.
    Text,
}

/// A chat row that can be filtered and rendered, abstracting over live
/// packet-derived messages (`ChatMessage`) and parsed history log lines
/// (`ParsedLogLine`). Live-only extras (stars, supporter badge) default to none
/// so history rows render without them.
trait ChatRow {
    fn chat_type(&self) -> ChatType;
    fn sender(&self) -> &str;
    fn recipient(&self) -> Option<&str>;
    fn text(&self) -> &str;
    /// Time-of-day string, `HH:MM:SS`.
    fn time_str(&self) -> String;
    fn stars(&self) -> i16 {
        0
    }
    fn is_supporter(&self) -> bool {
        false
    }
    fn is_outgoing(&self) -> bool {
        false
    }

    /// The player name shown for this row: the recipient for outgoing whispers
    /// (rendered "→ recipient"), otherwise the sender.
    fn displayed_name(&self) -> &str {
        if self.is_outgoing() {
            self.recipient().unwrap_or_else(|| self.sender())
        } else {
            self.sender()
        }
    }

    /// True when either participant name contains `query` (already lowercased).
    /// Checks both the sender and the recipient so every visible name is
    /// searchable regardless of message direction.
    fn name_contains(&self, query: &str) -> bool {
        if self.sender().to_ascii_lowercase().contains(query) {
            return true;
        }
        self.recipient()
            .is_some_and(|r| r.to_ascii_lowercase().contains(query))
    }

    /// Case-insensitive (ASCII) search match. `query` must be already lowercased
    /// and trimmed. `scope` selects which fields are searched.
    fn matches_search(&self, query: &str, scope: SearchScope) -> bool {
        match scope {
            SearchScope::All => {
                self.name_contains(query) || self.text().to_ascii_lowercase().contains(query)
            }
            SearchScope::Name => self.name_contains(query),
            SearchScope::Text => self.text().to_ascii_lowercase().contains(query),
        }
    }

    /// True if this row is part of a conversation with `name` (case-insensitive).
    /// For whispers, matches either participant; for other types, matches sender.
    fn matches_participant(&self, name: &str) -> bool {
        if self.sender().eq_ignore_ascii_case(name) {
            return true;
        }
        self.recipient()
            .is_some_and(|r| r.eq_ignore_ascii_case(name))
    }
}

/// A single chat message.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    /// Timestamp when the message was received
    pub timestamp: DateTime<Local>,
    /// Name of the sender
    pub sender: String,
    /// Message content
    pub text: String,
    /// Type of message (determines color)
    pub chat_type: ChatType,
    /// Recipient (for whispers)
    pub recipient: Option<String>,
    /// Number of stars of the sender
    pub stars: i16,
    /// Whether sender is a supporter
    pub is_supporter: bool,
    /// Boss object ID for boss spawn announcements (to display sprite)
    pub boss_id: Option<i32>,
    /// True if this is a whisper sent by the local player (render as "→ recipient: text")
    pub is_outgoing: bool,
}

impl ChatMessage {
    /// Create a new chat message from a TextPacket.
    pub fn from_text_packet(packet: &realmhound_core::protocol::TextPacket) -> Option<Self> {
        // Determine chat type based on packet data
        let chat_type = Self::determine_chat_type(packet);

        // Parse boss spawn JSON first (needed to filter Oryx messages)
        let (mut text, mut boss_id) = Self::parse_boss_spawn_text(&packet.text);

        // Filter announcements - only keep Oryx the Mad God, Mysterious Crystal,
        // and Commander Calbrik boss spawn messages.
        if chat_type == ChatType::Announcement {
            let is_oryx = packet.name.contains("Oryx the Mad God");
            let is_crystal = packet.name.contains("Mysterious Crystal");
            let is_calbrik = packet.name.contains("Commander Calbrik");
            let is_herman = packet.name.contains("Herman the Rodent Hunter");

            if !is_oryx && !is_crystal && !is_calbrik && !is_herman {
                return None;
            }

            // Commander Calbrik's "Fine, I'll do it myself..." is the real UFO
            // encounter spawn callout. Oryx's earlier announcement
            // uses the AI_Alien_Taunt stringlist key which resolves to an
            // invisible taunt controller — suppress that and use Calbrik's
            // message instead.
            if is_calbrik && packet.text.contains("Fine, I'll do it myself") {
                let asset_manager = get_asset_manager();
                boss_id = asset_manager
                    .object_id_for_name("AI Alien UFO Event")
                    .or_else(|| asset_manager.object_id_for_name("AI Alien UFO"))
                    .or_else(|| asset_manager.killer_sprite_id("Alien UFO"));
                if boss_id.is_some() {
                    text = "UFO".to_string();
                }
            }

            // Alien Invasion waves: Calbrik narrates each wave with a distinct
            // taunt. Route the matching wave to the Live Feed with its minion
            // sprite instead of dropping it as an unresolved announcement.
            if is_calbrik && boss_id.is_none() {
                if let Some((wave_name, minion_id_name)) = Self::alien_wave_override(&packet.text) {
                    boss_id = get_asset_manager().object_id_for_name(minion_id_name);
                    if boss_id.is_some() {
                        text = wave_name.to_string();
                    }
                }
            }

            // Mammoth Rat: the rare Rat Extermination Adept encounter has no
            // Oryx spawn announcement; Herman the Rodent Hunter narrates it.
            // Route it to the Live Feed with the Mammoth Rat sprite so it can
            // also trigger realm-event notifications.
            if is_herman
                && boss_id.is_none()
                && packet.text.to_lowercase().contains("colossal pres")
            {
                let asset_manager = get_asset_manager();
                boss_id = asset_manager
                    .object_id_for_name("Mammoth City Rat")
                    .or_else(|| asset_manager.object_id_for_name("Mammoth Rat"))
                    .or_else(|| asset_manager.killer_sprite_id("Mammoth Rat"));
                if boss_id.is_some() {
                    text = "Mammoth Rat".to_string();
                }
            }

            // Crystal callouts carry no resolvable boss_id, so synthesize the
            // Mysterious Crystal sprite + name. This routes them to the Live Feed
            // (via PushBossCall) like other encounter callouts, instead of leaving
            // them as a plain chat announcement.
            if is_crystal && boss_id.is_none() {
                let asset_manager = get_asset_manager();
                boss_id = asset_manager
                    .object_id_for_name("New Mysterious Crystal")
                    .or_else(|| asset_manager.object_id_for_name("Mysterious Crystal"))
                    .or_else(|| asset_manager.killer_sprite_id("Mysterious Crystal"));
                if boss_id.is_some() {
                    text = "Mysterious Crystal".to_string();
                }
            }

            // Only show boss spawn announcements (ones with boss_id) - except Crystal which may not have one
            if boss_id.is_none() && !is_crystal {
                return None;
            }
        }

        // Set recipient for whispers only (not for party/guild chat which also use the recipient field)
        let recipient = if chat_type == ChatType::Whisper
            && !packet.recipient.is_empty()
            && packet.recipient != "*Guild*"
        {
            Some(Self::clean_name(&packet.recipient))
        } else {
            None
        };

        // Clean up sender name:
        // - Remove # prefix from announcements (e.g., "#Oryx the Mad God" -> "Oryx the Mad God")
        // - Remove trailing metadata (e.g., "PlayerName,a19d" -> "PlayerName")
        let sender = if packet.name.starts_with('#') {
            Self::clean_name(packet.name.trim_start_matches('#'))
        } else {
            Self::clean_name(&packet.name)
        };

        // Clean up message text (remove trailing metadata like ";c")
        let text = Self::clean_text(&text);

        Some(Self {
            timestamp: Local::now(),
            sender,
            text,
            chat_type,
            recipient,
            stars: packet.num_stars,
            is_supporter: packet.is_supporter,
            boss_id,
            is_outgoing: false,
        })
    }

    /// Clean text by removing trailing metadata (comma followed by ID codes).
    /// Names come as "PlayerName,a19d" - we split by comma and take the first part.
    fn clean_name(name: &str) -> String {
        name.split(',').next().unwrap_or(name).to_string()
    }

    /// Clean message text by removing trailing metadata (semicolon followed by short codes).
    fn clean_text(text: &str) -> String {
        // Remove trailing patterns like ";c" - metadata suffixes added by the game client
        if let Some(pos) = text.rfind(';') {
            let suffix = &text[pos + 1..];
            // Only strip if suffix is short (metadata) and doesn't look like normal text
            if suffix.len() <= 5 && !suffix.contains(' ') {
                return text[..pos].to_string();
            }
        }
        text.to_string()
    }

    /// Curated overrides for encounter spawn announcements whose `stringlist`
    /// key resolves to the wrong object (an invisible spawner/controller or a
    /// joke placeholder name) or to no object at all.
    ///
    /// Keyed by the raw `stringlist.<key>.new.N` segment. Returns
    /// `(display_name, sprite_id_name)` where `sprite_id_name` is the
    /// `ObjectID.list` `id_name` of the object whose sprite should be shown.
    ///
    /// RealmHound does not load the game's `stringlist`
    /// localization asset, so these names cannot be resolved automatically.
    fn encounter_name_override(key: &str) -> Option<(&'static str, &'static str)> {
        match key {
            // Resolves to id 3408 whose display_name is a joke ("Lord of the Lol 0").
            "Lord_of_the_Lost_Lands" => Some(("Lord of the Lost Lands", "Lord of the Lost Lands")),
            // Spawns as "Behemoth's Egg"; should read as the boss with its sprite.
            "Flying_Behemoth_Egg" => Some(("Flying Behemoth", "Flying Behemoth")),
            // Combined Mountain Temple encounter with no single matching object.
            "Temple_Encounter" => Some(("Jade and Garnet Statues", "Jade Statue")),
            // Resolves to the invisible "Beer Encounter Spawner", not the boss.
            "Beer_Encounter_Spawner" => Some(("Beer God", "Beer God")),
            // Resolves to the invisible Epic Hive taunt controller, not the boss.
            "EH_Event_Taunt_Controller" => Some(("Killer Bee Nest", "EH Queen Blue Hive")),
            _ => None,
        }
    }

    /// Stringlist keys that are pre-event taunts rather than actual encounter
    /// spawns. These resolve to invisible controller objects and should NOT
    /// generate a Live Feed callout. The real spawn callout comes from the NPC
    /// (e.g. Commander Calbrik for the Alien Invasion).
    fn is_suppressed_encounter(key: &str) -> bool {
        matches!(key, "AI_Alien_Taunt")
    }

    /// Detect an Alien Invasion wave from Commander Calbrik's taunt text.
    ///
    /// Each wave of the Adept and Veteran invasions is announced by a distinct
    /// Calbrik line. Matching is done on a distinctive substring so straight vs.
    /// curly quotes and trailing metadata don't matter. Returns the Live Feed
    /// display name and the `ObjectID.list` `id_name` of the wave's minion whose
    /// sprite should be shown. Veteran waves 1 and 3 have no known trigger text.
    fn alien_wave_override(text: &str) -> Option<(&'static str, &'static str)> {
        if text.contains("Atmosphere breach imminent") {
            Some(("Alien Invasion Adept - Wave 1", "AI Alien Soldier"))
        } else if text.contains("extermination units") {
            Some(("Alien Invasion Adept - Wave 2", "AI Alien Mini Tank"))
        } else if text.contains("be faced with extreme prejudice") {
            Some(("Alien Invasion Adept - Wave 3", "AI Alien Tank"))
        } else if text.contains("Deploy punisher units") {
            Some(("Alien Invasion Adept - Wave 4", "AI Alien Pod"))
        } else if text.contains("The members of the Calbrik Guard will grant you") {
            Some(("Alien Invasion Veteran - Wave 1", "AI Alien Sage"))
        } else if text.contains("Release the Mech Squad") {
            Some(("Alien Invasion Veteran - Wave 2", "AI Alien Buster"))
        } else if text.contains("even those creepy pods") {
            Some(("Alien Invasion Veteran - Wave 3", "AI Alien Destroyer"))
        } else if text.contains("How can eliminating such simple humanoid creatures") {
            Some(("Alien Invasion Veteran - Wave 4", "AI Alien Spoder"))
        } else {
            None
        }
    }

    /// Parse boss spawn text from Oryx announcements.
    /// Detects JSON-like strings like `{"k":"stringlist.Sigma_Werewolf.new.0"}`
    /// and extracts the boss name to look up the sprite.
    /// Returns (display_text, optional_boss_id).
    fn parse_boss_spawn_text(text: &str) -> (String, Option<i32>) {
        // Look for JSON-like boss spawn pattern: {"k":"stringlist.BOSS_NAME.new.0"}
        // Example: "The Plague Doctor{"k":"stringlist.The_Plague_Doctor.new.0"}"
        if let Some(json_start) = text.find(r#"{"k":"stringlist."#) {
            if let Some(name_end_offset) = text[json_start..].find(r#".new."#) {
                // Extract boss id_name between "stringlist." and ".new."
                let prefix_len = r#"{"k":"stringlist."#.len();
                let boss_name_start = json_start + prefix_len;
                let boss_name_end = json_start + name_end_offset;

                if boss_name_end > boss_name_start {
                    let boss_id_name_raw = &text[boss_name_start..boss_name_end];

                    // Some stringlist keys are pre-event taunts, not
                    // actual encounter spawns. Suppress them so they don't create
                    // a Live Feed entry (the real callout comes from the NPC).
                    if Self::is_suppressed_encounter(boss_id_name_raw) {
                        return (text.to_string(), None);
                    }

                    let asset_manager = get_asset_manager();

                    // Curated overrides take priority. The default
                    // id_name resolution below mis-resolves several encounters
                    // (invisible spawners, joke placeholder names, or combined
                    // encounters with no single object).
                    if let Some((override_name, sprite_id_name)) =
                        Self::encounter_name_override(boss_id_name_raw)
                    {
                        let boss_id = asset_manager
                            .object_id_for_name(sprite_id_name)
                            .or_else(|| asset_manager.killer_sprite_id(sprite_id_name));
                        return (override_name.to_string(), boss_id);
                    }

                    // The JSON uses underscores, but the asset names use spaces
                    // e.g., "Sigma_Werewolf" in JSON -> "Sigma Werewolf" in assets
                    // Also handle the WorldAPOSs -> World's conversion
                    let boss_id_name = boss_id_name_raw
                        .replace('_', " ")
                        .replace("APOSs", "'s")
                        .replace("APOS", "'");

                    // Look up the boss ID in asset manager
                    let boss_id = asset_manager.object_id_for_name(&boss_id_name);

                    // Get display name - use text before JSON if available, otherwise use asset name
                    let display_name = if json_start > 0 {
                        // Text before JSON is the display name (e.g., "The Plague Doctor")
                        text[..json_start].trim().to_string()
                    } else if let Some(id) = boss_id {
                        // Use asset display name
                        asset_manager
                            .object_name(id)
                            .unwrap_or_else(|| boss_id_name.clone())
                    } else {
                        // Fallback to cleaned id_name
                        boss_id_name.clone()
                    };

                    return (display_name, boss_id);
                }
            }
        }

        // No boss spawn JSON found, return original text
        (text.to_string(), None)
    }

    /// Determine chat type from packet data.
    fn determine_chat_type(packet: &realmhound_core::protocol::TextPacket) -> ChatType {
        // Check for guild chat (name starts with #g:, or recipient is the guild marker)
        if packet.name.starts_with("#g:") || packet.name == "Guild" || packet.recipient == "*Guild*"
        {
            return ChatType::Guild;
        }

        // Check for party chat - either by name prefix or recipient containing "Party"
        if packet.name.starts_with("#p:")
            || packet.name == "Party"
            || packet.recipient.contains("Party")
        {
            return ChatType::Party;
        }

        // Check for whisper (has recipient, but not party)
        if !packet.recipient.is_empty() {
            return ChatType::Whisper;
        }

        // Check for server announcements (special names or negative object ID)
        if packet.name.starts_with('#') || packet.object_id < 0 {
            return ChatType::Announcement;
        }

        // Check for enemy/NPC (often has specific patterns)
        if packet.name.starts_with("Enemy") || packet.object_id < 0 {
            return ChatType::Enemy;
        }

        ChatType::Normal
    }
}

/// Maximum number of history entries kept in memory after a load.
const HISTORY_MAX_ENTRIES: usize = 100_000;
/// Number of entries rendered on each side of a "Find in chat" target so the
/// jumped-to line is shown in context without rendering the whole history.
const HISTORY_JUMP_WINDOW: usize = 1_000;
/// Safety cap on the number of daily log files scanned for a single range.
const HISTORY_MAX_DAYS: usize = 400;

/// Predefined rolling date ranges for browsing chat history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryRange {
    /// Just today's log.
    Today,
    /// The last 7 days.
    LastWeek,
    /// The last 30 days.
    LastMonth,
    /// The last 365 days.
    LastYear,
    /// Every available log file.
    AllTime,
    /// A user-picked start/end range.
    Custom,
}

/// The resolved selection actually loaded from disk, used as a cache key so we
/// only re-read files when the effective range changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoadedSel {
    /// Inclusive day range.
    Range(NaiveDate, NaiveDate),
    /// All available log files.
    All,
}

/// A single chat log line parsed from a persisted daily log file.
/// The date is not part of this struct; it comes from the log file name.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedLogLine {
    /// Time-of-day string as stored (`HH:MM:SS`).
    time: String,
    /// Chat type (only Normal/Guild/Party/Whisper are ever logged).
    chat_type: ChatType,
    /// Sender player name.
    sender: String,
    /// Whisper recipient, if this is a whisper line.
    recipient: Option<String>,
    /// Message body.
    text: String,
    /// Star rank of the sender, if persisted in the log line (0 otherwise).
    stars: i16,
    /// Whether the sender was a supporter, if persisted in the log line.
    is_supporter: bool,
}

impl ChatRow for ParsedLogLine {
    fn chat_type(&self) -> ChatType {
        self.chat_type
    }
    fn sender(&self) -> &str {
        &self.sender
    }
    fn recipient(&self) -> Option<&str> {
        self.recipient.as_deref()
    }
    fn text(&self) -> &str {
        &self.text
    }
    fn time_str(&self) -> String {
        self.time.clone()
    }
    fn stars(&self) -> i16 {
        self.stars
    }
    fn is_supporter(&self) -> bool {
        self.is_supporter
    }
}

impl ChatRow for ChatMessage {
    fn chat_type(&self) -> ChatType {
        self.chat_type
    }
    fn sender(&self) -> &str {
        &self.sender
    }
    fn recipient(&self) -> Option<&str> {
        self.recipient.as_deref()
    }
    fn text(&self) -> &str {
        &self.text
    }
    fn time_str(&self) -> String {
        self.timestamp.format("%H:%M:%S").to_string()
    }
    fn stars(&self) -> i16 {
        self.stars
    }
    fn is_supporter(&self) -> bool {
        self.is_supporter
    }
    fn is_outgoing(&self) -> bool {
        self.is_outgoing
    }
}

/// A parsed history line together with the date it was logged.
#[derive(Debug, Clone)]
struct HistoryEntry {
    date: NaiveDate,
    line: ParsedLogLine,
}

/// Parse a single persisted chat log line into its parts. Returns `None` for
/// empty or malformed lines so callers can tolerantly skip them.
///
/// Expected formats (the arrow is U+2192 with surrounding spaces):
///   `HH:MM:SS [PUBLIC|GUILD|PARTY] sender: text`
///   `HH:MM:SS [WHISPER] sender → recipient: text`
///
/// An optional metadata prefix may appear between the type tag and the sender:
/// `★N` for the sender's star rank and `♦` for supporter status, e.g.
///   `HH:MM:SS [PUBLIC] ★12 ♦ sender: text`
/// Lines without the prefix (older logs) still parse, with stars/supporter unset.
fn parse_log_line(line: &str) -> Option<ParsedLogLine> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.trim().is_empty() {
        return None;
    }

    // Time, validated by an actual parse (rejects e.g. "99:99:99").
    let (time, rest) = line.split_once(' ')?;
    NaiveTime::parse_from_str(time, "%H:%M:%S").ok()?;

    // Type tag in brackets.
    let rest = rest.trim_start().strip_prefix('[')?;
    let close = rest.find(']')?;
    let chat_type = match &rest[..close] {
        "PUBLIC" => ChatType::Normal,
        "GUILD" => ChatType::Guild,
        "PARTY" => ChatType::Party,
        "WHISPER" => ChatType::Whisper,
        _ => return None,
    };

    // Optional metadata prefix (★N star rank, ♦ supporter) between the type tag
    // and the sender. Player names never begin with these markers, so stripping
    // them here is unambiguous.
    let mut body = rest[close + 1..].trim_start();
    let mut stars: i16 = 0;
    let mut is_supporter = false;
    loop {
        if let Some(after) = body.strip_prefix('★') {
            let digits = after.trim_start();
            let end = digits
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(digits.len());
            if end > 0 {
                stars = digits[..end].parse().unwrap_or(0);
                body = digits[end..].trim_start();
                continue;
            }
        }
        if let Some(after) = body.strip_prefix('♦') {
            is_supporter = true;
            body = after.trim_start();
            continue;
        }
        break;
    }

    let (sender, recipient, text) = if chat_type == ChatType::Whisper {
        // Preferred format "sender → recipient: text". Player names contain no
        // spaces, so a valid pre-arrow part is a bare sender name.
        let two_party = body.split_once(" → ").and_then(|(sender, after)| {
            let (recipient, text) = after.split_once(": ")?;
            if sender.trim().is_empty() || recipient.trim().is_empty() {
                return None;
            }
            Some((
                sender.to_string(),
                Some(recipient.to_string()),
                text.to_string(),
            ))
        });
        if let Some(parsed) = two_party {
            parsed
        } else {
            // Legacy/incoming whispers were logged without a recipient
            // (`[WHISPER] sender: text`). Accept only a bare sender name (no
            // whitespace) so genuinely malformed lines are still rejected.
            let (sender, text) = body.split_once(": ")?;
            if sender.trim().is_empty() || sender.contains(char::is_whitespace) {
                return None;
            }
            (sender.to_string(), None, text.to_string())
        }
    } else {
        // Names contain no ": ", so the first ": " terminates the sender name.
        let (sender, text) = body.split_once(": ")?;
        if sender.trim().is_empty() {
            return None;
        }
        (sender.to_string(), None, text.to_string())
    };

    Some(ParsedLogLine {
        time: time.to_string(),
        chat_type,
        sender,
        recipient,
        text,
        stars,
        is_supporter,
    })
}

/// Chat panel state and UI.
pub struct ChatPanel {
    /// Filter settings for each chat type (persisted to settings).
    filters: ChatFilters,
    /// Selected predefined history range.
    history_range: HistoryRange,
    /// Custom range start (used when `history_range == Custom`).
    history_start: NaiveDate,
    /// Custom range end (used when `history_range == Custom`).
    history_end: NaiveDate,
    /// Case-insensitive search query for the history browser.
    history_search: String,
    /// Which fields the history search matches against.
    history_search_scope: SearchScope,
    /// Active sender filter for history, set by clicking a name.
    history_sender_filter: Option<String>,
    /// Parsed history entries loaded from disk for the current selection.
    history_entries: Vec<HistoryEntry>,
    /// The selection currently loaded; used to avoid re-reading files every frame.
    history_loaded: Option<LoadedSel>,
    /// True when the entry cap was hit during the last load.
    history_truncated: bool,
    /// Error message from the last load attempt, if any.
    history_error: Option<String>,
    /// Pending "Find in chat" target: entry index to keep in view (with context).
    history_jump_to: Option<usize>,
    /// Whether to scroll to `history_jump_to` on the next frame (one-shot).
    history_jump_scroll: bool,
    /// Entry index currently flashing after a jump, with its flash deadline.
    history_jump_flash: Option<(usize, Instant)>,
    /// One-shot request to scroll the list to the bottom (newest) after a
    /// (re)load, so the latest messages are shown on entry and after Refresh.
    history_scroll_to_bottom: bool,
    /// Transient green status line (e.g. "Copied: ..."), with the time it was set.
    status_message: Option<(String, Instant)>,
    /// Per-account chat log directory. Injected so the panel resolves no path
    /// itself; an empty path disables logging and history.
    log_dir: PathBuf,
}

/// Filter settings for chat types.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatFilters {
    pub show_normal: bool,
    pub show_guild: bool,
    pub show_party: bool,
    pub show_whisper: bool,
    pub show_announcement: bool,
}

impl Default for ChatFilters {
    fn default() -> Self {
        Self {
            show_normal: true,
            show_guild: true,
            show_party: true,
            show_whisper: true,
            show_announcement: true,
        }
    }
}

impl ChatFilters {
    /// Check if a chat type should be shown.
    pub fn should_show(&self, chat_type: ChatType) -> bool {
        match chat_type {
            ChatType::Normal => self.show_normal,
            ChatType::Guild => self.show_guild,
            ChatType::Party => self.show_party,
            ChatType::Whisper => self.show_whisper,
            ChatType::Announcement => self.show_announcement,
            ChatType::Enemy => true, // Always show enemy/boss messages
        }
    }
}

impl Default for ChatPanel {
    fn default() -> Self {
        Self {
            filters: ChatFilters::default(),
            history_range: HistoryRange::Today,
            history_start: Local::now().date_naive() - chrono::Duration::days(30),
            history_end: Local::now().date_naive(),
            history_search: String::new(),
            history_search_scope: SearchScope::default(),
            history_sender_filter: None,
            history_entries: Vec::new(),
            history_loaded: None,
            history_truncated: false,
            history_error: None,
            history_jump_to: None,
            history_jump_scroll: false,
            history_jump_flash: None,
            history_scroll_to_bottom: true,
            status_message: None,
            log_dir: PathBuf::new(),
        }
    }
}

impl ChatPanel {
    /// Create a new chat panel with settings loaded from ChatSettings and an
    /// explicit per-account chat log directory.
    pub fn new_with_settings(
        settings: &realmhound_core::settings::ChatSettings,
        log_dir: PathBuf,
    ) -> Self {
        let filters = ChatFilters {
            show_normal: settings.show_public,
            show_guild: settings.show_guild,
            show_party: settings.show_party,
            show_whisper: settings.show_whisper,
            show_announcement: true, // Always show announcements
        };

        Self {
            filters,
            log_dir,
            ..Self::default()
        }
    }

    /// Get a reference to the current filters (for saving).
    pub fn filters(&self) -> &ChatFilters {
        &self.filters
    }

    /// Reload the chat history from disk and scroll to the newest messages on the
    /// next frame. Called when the Chat tab is (re)activated so it always opens on
    /// the latest messages, and picks up anything logged since it was last viewed.
    pub fn reset_scroll(&mut self) {
        self.history_loaded = None;
        self.clear_history_jump();
        self.history_scroll_to_bottom = true;
    }

    /// Render the transient green status line (e.g. "Copied: ...") if one was
    /// set within the last 3 seconds, otherwise clear it.
    fn show_status_line(&mut self, ui: &mut egui::Ui) {
        if let Some((msg, time)) = &self.status_message {
            if time.elapsed().as_secs() < 3 {
                ui.label(RichText::new(msg).color(Color32::GREEN));
                ui.ctx().request_repaint();
            } else {
                self.status_message = None;
            }
        }
    }

    /// Filter the history to messages involving a specific sender.
    /// Called by the app when processing `AppAction::ViewChatWith`. Ensures the
    /// whisper filter is on so the conversation is visible, without disturbing the
    /// user's other persisted type-filter preferences.
    pub fn view_chat_with(&mut self, sender: String) {
        self.history_range = HistoryRange::AllTime;
        self.history_sender_filter = Some(sender);
        self.history_search = String::new();
        self.filters.show_whisper = true;
        self.history_loaded = None;
    }

    /// Get color for star count based on RotMG star tiers.
    fn star_color(stars: i16) -> Color32 {
        let [r, g, b] = realmhound_core::account_stats::star_color(stars as i32);
        Color32::from_rgb(r, g, b)
    }

    /// Build a `LayoutJob` for `text` in `color`, highlighting case-insensitive (ASCII)
    /// matches of `query` (already lowercased). Returns plain text when `query` is empty.
    fn highlight_job(text: &str, query: &str, color: Color32) -> LayoutJob {
        let fmt = |bg: Option<Color32>| {
            let mut f = TextFormat {
                color,
                ..Default::default()
            };
            if let Some(b) = bg {
                f.background = b;
            }
            f
        };

        let mut job = LayoutJob::default();
        if query.is_empty() {
            job.append(text, 0.0, fmt(None));
            return job;
        }

        // to_ascii_lowercase preserves byte length, so offsets map back to `text`.
        let lower = text.to_ascii_lowercase();
        let mut start = 0;
        while let Some(off) = lower[start..].find(query) {
            let mpos = start + off;
            if mpos > start {
                job.append(&text[start..mpos], 0.0, fmt(None));
            }
            let mend = mpos + query.len();
            job.append(&text[mpos..mend], 0.0, fmt(Some(SEARCH_HIGHLIGHT)));
            start = mend;
        }
        if start < text.len() {
            job.append(&text[start..], 0.0, fmt(None));
        }
        job
    }

    /// Render the clickable player-name label as `[name]` with an in-game style
    /// black outline, returning its rect (for click hit-testing). The brackets and
    /// name use `color`; search matches are highlighted as elsewhere. The brackets
    /// are visual only -- callers still copy/filter by the raw name.
    fn name_label(ui: &mut Ui, name: &str, color: Color32, search: &str) -> egui::Rect {
        let display = format!("[{name}]");
        // Top layer: colored name (with optional search highlight).
        let top = if search.is_empty() {
            egui::WidgetText::from(RichText::new(&display).color(color).strong()).into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Body,
            )
        } else {
            egui::WidgetText::from(Self::highlight_job(&display, search, color)).into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Body,
            )
        };
        // Outline layer: identical glyph layout in solid black. Search
        // highlighting doesn't affect the silhouette, so a plain black galley
        // aligns exactly under the top layer.
        let outline = egui::WidgetText::from(RichText::new(&display).color(NAME_OUTLINE).strong())
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Body,
            );

        let (rect, _) = ui.allocate_exact_size(top.size(), Sense::hover());
        let painter = ui.painter();
        for (dx, dy) in [
            (-1.0, 0.0),
            (1.0, 0.0),
            (0.0, -1.0),
            (0.0, 1.0),
            (-1.0, -1.0),
            (1.0, -1.0),
            (-1.0, 1.0),
            (1.0, 1.0),
        ] {
            painter.galley(rect.min + egui::vec2(dx, dy), outline.clone(), NAME_OUTLINE);
        }
        painter.galley(rect.min, top.clone(), color);
        rect
    }

    /// Render the message body. Highlights search matches only when `highlight`
    /// is set (i.e. the search scope includes message text).
    /// Renders `<sprite name=X>` tags as inline emote icons.
    fn body_label(
        ui: &mut Ui,
        text: &str,
        color: Color32,
        search: &str,
        highlight: bool,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        if !emotes::has_emote_tags(text) {
            if search.is_empty() || !highlight {
                ui.label(RichText::new(text).color(color));
            } else {
                ui.label(Self::highlight_job(text, search, color));
            }
            return;
        }

        let segments = emotes::parse_segments(text);
        let asset_manager = get_asset_manager();

        let prev_spacing = ui.spacing().item_spacing.x;
        ui.spacing_mut().item_spacing.x = 0.0;

        for segment in segments {
            match segment {
                EmoteSegment::Text(t) if !t.is_empty() => {
                    if search.is_empty() || !highlight {
                        ui.label(RichText::new(t).color(color));
                    } else {
                        ui.label(Self::highlight_job(t, search, color));
                    }
                }
                EmoteSegment::Emote(name) => {
                    let object_id = emotes::resolve_emote_id(asset_manager, name);
                    let mut drawn = false;

                    if let Some(id) = object_id {
                        ui.add_space(EMOTE_PADDING);
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(EMOTE_SPRITE_SIZE, EMOTE_SPRITE_SIZE),
                            Sense::hover(),
                        );
                        drawn = sprite_renderer.draw_sprite_in_rect(ui, id, rect);
                        ui.add_space(EMOTE_PADDING);
                    }

                    if !drawn {
                        ui.label(RichText::new(emotes::fallback_text(name)).color(color));
                    }
                }
                _ => {}
            }
        }

        ui.spacing_mut().item_spacing.x = prev_spacing;
    }

    /// Render the search-scope selector (shared by the live header and the
    /// history controls). `id` must be unique per call site.
    fn search_scope_combo(
        ui: &mut Ui,
        shadcn: &crate::shadcn_ui::Shadcn,
        id: &str,
        scope: &mut SearchScope,
    ) {
        ui.label("in:");
        let mut key = Some(
            match *scope {
                SearchScope::All => "all",
                SearchScope::Name => "name",
                SearchScope::Text => "text",
            }
            .to_string(),
        );
        shadcn.sel(
            ui,
            id,
            &mut key,
            70.0,
            &[("all", "All"), ("name", "Name"), ("text", "Text")],
        );
        if let Some(k) = &key {
            *scope = match k.as_str() {
                "name" => SearchScope::Name,
                "text" => SearchScope::Text,
                _ => SearchScope::All,
            };
        }
    }

    /// Render the four chat-type filter toggle buttons (shared by the live header and
    /// the history controls). Each label is colored to match its chat type.
    fn type_filter_buttons(
        ui: &mut Ui,
        shadcn: &crate::shadcn_ui::Shadcn,
        filters: &mut ChatFilters,
    ) {
        shadcn.tgl(
            ui,
            &mut filters.show_normal,
            RichText::new("Normal").color(ChatType::Normal.color()),
        );
        shadcn.tgl(
            ui,
            &mut filters.show_guild,
            RichText::new("Guild").color(ChatType::Guild.color()),
        );
        shadcn.tgl(
            ui,
            &mut filters.show_party,
            RichText::new("Party").color(ChatType::Party.color()),
        );
        shadcn.tgl(
            ui,
            &mut filters.show_whisper,
            RichText::new("Whisper").color(ChatType::Whisper.color()),
        );
    }

    /// Render a single chat row's contents (within a horizontal layout), shared by
    /// the live and history views. When `date` is `Some`, it is prefixed to the
    /// timestamp (used by multi-day history ranges). Returns the rect of the
    /// clickable player-name label (used for click-to-filter).
    fn render_row(
        ui: &mut Ui,
        row: &dyn ChatRow,
        date: Option<NaiveDate>,
        search: &str,
        scope: SearchScope,
        sprite_renderer: &mut SpriteRenderer,
    ) -> egui::Rect {
        // Highlight names unless searching text-only; highlight the body unless
        // searching name-only.
        let name_search = if scope == SearchScope::Text {
            ""
        } else {
            search
        };
        let highlight_body = scope != SearchScope::Name;
        // Timestamp, optionally prefixed with the date for multi-day ranges.
        let time = match date {
            Some(d) => format!("{} {}", d.format("%Y-%m-%d"), row.time_str()),
            None => row.time_str(),
        };
        ui.label(RichText::new(time).color(Color32::GRAY));

        // Stars (live only; history rows report 0).
        if row.stars() > 0 {
            ui.label(
                RichText::new(format!("★{}", row.stars())).color(Self::star_color(row.stars())),
            );
        }

        // Supporter badge (live only).
        if row.is_supporter() {
            ui.label(RichText::new("♦").color(Color32::from_rgb(255, 100, 100)));
        }

        let color = row.chat_type().color();
        // Names stand out like in-game chat: yellow for public chat, otherwise the
        // channel color so they match the rest of the message.
        let name_color = if row.chat_type() == ChatType::Normal {
            NAME_YELLOW
        } else {
            color
        };
        match row.recipient() {
            Some(recipient) => {
                // Whisper: always "sender → recipient: text". Return the
                // conversation partner's name rect (recipient for outgoing,
                // sender otherwise) so click-to-filter matches `displayed_name`.
                let sender_rect = Self::name_label(ui, row.sender(), name_color, name_search);
                ui.label(RichText::new("→").color(Color32::GRAY));
                let recipient_rect = Self::name_label(ui, recipient, name_color, name_search);
                ui.label(RichText::new(":").color(Color32::GRAY));
                Self::body_label(
                    ui,
                    row.text(),
                    color,
                    search,
                    highlight_body,
                    sprite_renderer,
                );
                if row.is_outgoing() {
                    recipient_rect
                } else {
                    sender_rect
                }
            }
            None => {
                let name_rect = Self::name_label(ui, row.sender(), name_color, name_search);
                ui.label(RichText::new(":").color(Color32::GRAY));
                Self::body_label(
                    ui,
                    row.text(),
                    color,
                    search,
                    highlight_body,
                    sprite_renderer,
                );
                name_rect
            }
        }
    }

    /// Persist a new chat message to the daily log file. The unified chat view is
    /// disk-backed and refreshes on tab entry / Refresh, so nothing is retained in
    /// memory here.
    pub fn add_message(&mut self, message: ChatMessage) {
        self.log_to_file(&message);
    }

    /// The per-account chat log directory, or `None` when unset.
    fn chat_log_dir(&self) -> Option<&Path> {
        if self.log_dir.as_os_str().is_empty() {
            None
        } else {
            Some(self.log_dir.as_path())
        }
    }

    /// Log a chat message to the daily log file.
    fn log_to_file(&self, message: &ChatMessage) {
        // Only log normal, guild, party, and whisper chats
        match message.chat_type {
            ChatType::Normal | ChatType::Guild | ChatType::Party | ChatType::Whisper => {}
            _ => return,
        }

        let Some(log_dir) = self.chat_log_dir() else {
            return;
        };

        // Create log directory if it doesn't exist
        if let Err(e) = fs::create_dir_all(&log_dir) {
            tracing::warn!("Failed to create chat log directory: {}", e);
            return;
        }

        // Create daily log file name
        let date_str = Local::now().format("%Y-%m-%d").to_string();
        let log_path = log_dir.join(format!("chat_{}.log", date_str));

        // Format log line
        let time_str = message.timestamp.format("%H:%M:%S").to_string();
        let type_str = match message.chat_type {
            ChatType::Normal => "[PUBLIC]",
            ChatType::Guild => "[GUILD]",
            ChatType::Party => "[PARTY]",
            ChatType::Whisper => "[WHISPER]",
            _ => return,
        };

        // Optional metadata prefix so history can render the sender's star rank
        // and supporter badge (parsed back by `parse_log_line`).
        let mut meta = String::new();
        if message.stars > 0 {
            meta.push_str(&format!("★{} ", message.stars));
        }
        if message.is_supporter {
            meta.push_str("♦ ");
        }

        let log_line = match &message.recipient {
            Some(recipient) => format!(
                "{} {} {}{} → {}: {}\n",
                time_str, type_str, meta, message.sender, recipient, message.text
            ),
            None => format!(
                "{} {} {}{}: {}\n",
                time_str, type_str, meta, message.sender, message.text
            ),
        };

        // Append to log file
        match OpenOptions::new().create(true).append(true).open(&log_path) {
            Ok(mut file) => {
                if let Err(e) = file.write_all(log_line.as_bytes()) {
                    tracing::warn!("Failed to write to chat log: {}", e);
                }
            }
            Err(e) => {
                tracing::warn!("Failed to open chat log file: {}", e);
            }
        }
    }

    /// Render the chat panel UI. The Live and History views are unified into a
    /// single disk-backed browser.
    /// Returns true if filters were changed and settings should be saved.
    pub fn render(
        &mut self,
        ui: &mut Ui,
        shadcn: &crate::shadcn_ui::Shadcn,
        sprite_renderer: &mut SpriteRenderer,
    ) -> bool {
        let filters_before = self.filters.clone();
        self.render_history(ui, shadcn, sprite_renderer);
        self.filters != filters_before
    }

    /// Clear any active "Find in chat" history jump (target, scroll, flash).
    fn clear_history_jump(&mut self) {
        self.history_jump_to = None;
        self.history_jump_scroll = false;
        self.history_jump_flash = None;
    }

    /// Resolve the current `HistoryRange` selection into a concrete `LoadedSel`.
    fn current_selection(&self) -> LoadedSel {
        let today = Local::now().date_naive();
        let rolling = |days: i64| {
            let start = today
                .checked_sub_signed(chrono::Duration::days(days))
                .unwrap_or(today);
            LoadedSel::Range(start, today)
        };
        match self.history_range {
            HistoryRange::Today => LoadedSel::Range(today, today),
            HistoryRange::LastWeek => rolling(6),
            HistoryRange::LastMonth => rolling(29),
            HistoryRange::LastYear => rolling(364),
            HistoryRange::AllTime => LoadedSel::All,
            HistoryRange::Custom => {
                let start = self.history_start.min(self.history_end);
                let end = self.history_start.max(self.history_end);
                LoadedSel::Range(start, end)
            }
        }
    }

    /// Load and parse persisted chat logs for the given selection into memory.
    /// Applies chat-type and sender filters during loading so the entry cap
    /// only counts rows the user actually wants to see.
    fn load_history(&mut self, sel: LoadedSel) {
        self.history_entries.clear();
        self.history_truncated = false;
        self.history_error = None;

        let Some(log_dir) = self.chat_log_dir().map(Path::to_path_buf) else {
            self.history_error = Some("Could not locate the chat log directory.".to_string());
            return;
        };

        let filters = &self.filters;
        let sender_filter = &self.history_sender_filter;

        // Build the ordered list of (date, path) files to read.
        let mut files: Vec<(NaiveDate, PathBuf)> = Vec::new();
        match sel {
            LoadedSel::Range(start, end) => {
                let mut days = 0usize;
                for day in start.iter_days() {
                    if day > end || days >= HISTORY_MAX_DAYS {
                        break;
                    }
                    days += 1;
                    files.push((
                        day,
                        log_dir.join(format!("chat_{}.log", day.format("%Y-%m-%d"))),
                    ));
                }
            }
            LoadedSel::All => {
                if let Ok(read_dir) = fs::read_dir(&log_dir) {
                    for entry in read_dir.flatten() {
                        let path = entry.path();
                        let date = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .and_then(|n| n.strip_prefix("chat_"))
                            .and_then(|n| n.strip_suffix(".log"))
                            .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
                        if let Some(date) = date {
                            files.push((date, path));
                        }
                    }
                }
                files.sort_by_key(|(date, _)| *date);
                if files.len() > HISTORY_MAX_DAYS {
                    // Keep the most recent days when there are very many files.
                    files.drain(0..files.len() - HISTORY_MAX_DAYS);
                }
            }
        }

        'outer: for (date, path) in files {
            let Ok(contents) = fs::read_to_string(&path) else {
                continue;
            };
            for line in contents.lines() {
                if self.history_entries.len() >= HISTORY_MAX_ENTRIES {
                    self.history_truncated = true;
                    break 'outer;
                }
                if let Some(parsed) = parse_log_line(line) {
                    if !filters.should_show(parsed.chat_type) {
                        continue;
                    }
                    if let Some(sf) = sender_filter {
                        if !parsed.matches_participant(sf) {
                            continue;
                        }
                    }
                    self.history_entries
                        .push(HistoryEntry { date, line: parsed });
                }
            }
        }
    }

    /// Render the chat history browser (date range, search, results list).
    fn render_history(
        &mut self,
        ui: &mut Ui,
        shadcn: &crate::shadcn_ui::Shadcn,
        sprite_renderer: &mut SpriteRenderer,
    ) {
        // Keep custom dates ordered so the pickers always reflect a valid range.
        if self.history_start > self.history_end {
            std::mem::swap(&mut self.history_start, &mut self.history_end);
        }

        // Snapshot filter/search inputs so we can detect manual changes this frame
        // and drop any active jump (a stale context window must not constrain new
        // results).
        let prev_search = self.history_search.clone();
        let prev_scope = self.history_search_scope;
        let prev_filters = self.filters.clone();
        let prev_sender = self.history_sender_filter.clone();

        // Compute the message-count label from last frame's loaded entries/filters,
        // so it can be shown at the very left of the toolbar (matching the Live tab).
        let count_query = self.history_search.trim().to_ascii_lowercase();
        let count_has_search = !count_query.is_empty();
        let count_filtering_inputs = count_has_search || self.history_sender_filter.is_some();
        let count_passes = |e: &HistoryEntry| -> bool {
            if !self.filters.should_show(e.line.chat_type) {
                return false;
            }
            if let Some(sf) = &self.history_sender_filter {
                if !e.line.matches_participant(sf) {
                    return false;
                }
            }
            if count_has_search
                && !e
                    .line
                    .matches_search(&count_query, self.history_search_scope)
            {
                return false;
            }
            true
        };
        let count_matched = self
            .history_entries
            .iter()
            .filter(|e| count_passes(e))
            .count();
        let count_filtering = count_filtering_inputs || count_matched != self.history_entries.len();
        let count_label = if count_filtering {
            format!(
                "{} of {} entries",
                count_matched,
                self.history_entries.len()
            )
        } else {
            format!("{} entries loaded", self.history_entries.len())
        };

        // Controls row: message count, chat-type filters, range selector, optional
        // custom pickers, refresh, and search, all combined into a single toolbar
        // row (Message count | type filters | everything else, matching Live).
        shadcn.header_band_stacked_divided(ui, shadcn.secondary_header_fill(), |ui| {
            shadcn.band_row(ui, |ui| {
                ui.label(count_label);
                ui.separator();

                Self::type_filter_buttons(ui, shadcn, &mut self.filters);
                ui.separator();

                ui.label("From:");
                let mut range_key = Some(
                    match self.history_range {
                        HistoryRange::Today => "today",
                        HistoryRange::LastWeek => "last_week",
                        HistoryRange::LastMonth => "last_month",
                        HistoryRange::LastYear => "last_year",
                        HistoryRange::AllTime => "all_time",
                        HistoryRange::Custom => "custom",
                    }
                    .to_string(),
                );
                shadcn.sel(
                    ui,
                    "chat_history_range",
                    &mut range_key,
                    100.0,
                    &[
                        ("today", "Today"),
                        ("last_week", "Last week"),
                        ("last_month", "Last month"),
                        ("last_year", "Last year"),
                        ("all_time", "All time"),
                        ("custom", "Custom"),
                    ],
                );
                if let Some(key) = &range_key {
                    self.history_range = match key.as_str() {
                        "today" => HistoryRange::Today,
                        "last_week" => HistoryRange::LastWeek,
                        "last_month" => HistoryRange::LastMonth,
                        "last_year" => HistoryRange::LastYear,
                        "all_time" => HistoryRange::AllTime,
                        "custom" => HistoryRange::Custom,
                        _ => self.history_range,
                    };
                }

                if self.history_range == HistoryRange::Custom {
                    shadcn.date_range_picker(
                        ui,
                        "chat_hist_range",
                        &mut self.history_start,
                        &mut self.history_end,
                    );
                }

                ui.separator();
                if shadcn
                    .btn(ui, "⟳")
                    .hover_tip("Refresh (reload from disk)")
                    .clicked()
                {
                    self.history_loaded = None;
                }

                ui.separator();
                ui.label("🔍");
                ui.add(
                    egui::TextEdit::singleline(&mut self.history_search)
                        .id_source("chat_history_search_box")
                        .desired_width(160.0)
                        .hint_text("Filter history…"),
                );
                if !self.history_search.is_empty()
                    && shadcn.btn(ui, "✕").hover_tip("Clear search").clicked()
                {
                    self.history_search.clear();
                }
                Self::search_scope_combo(
                    ui,
                    shadcn,
                    "history_search_scope",
                    &mut self.history_search_scope,
                );

                if let Some(sf) = self.history_sender_filter.clone() {
                    ui.separator();
                    if shadcn
                        .button_sm(ui, format!("sender: {sf} ✕"))
                        .hover_tip("Clear sender filter")
                        .clicked()
                    {
                        self.history_sender_filter = None;
                    }
                }
            });
        });

        // Drop any active jump when the user changes a filter/search input, so the
        // context window does not constrain new results. Name-click sender changes
        // are applied after the loop and clear the jump there.
        if self.history_search != prev_search
            || self.history_search_scope != prev_scope
            || self.filters != prev_filters
            || self.history_sender_filter != prev_sender
        {
            self.clear_history_jump();
        }

        // Chat-type and sender filters are applied during loading so the entry
        // cap counts only matching rows. Invalidate the cache when they change.
        if self.filters != prev_filters || self.history_sender_filter != prev_sender {
            self.history_loaded = None;
        }

        // Lazy-load: only re-read files when the effective selection changes.
        let sel = self.current_selection();
        if self.history_loaded != Some(sel) {
            self.load_history(sel);
            self.history_loaded = Some(sel);
            // Entry indices are invalidated by a reload.
            self.clear_history_jump();
            // Show the newest messages after any (re)load.
            self.history_scroll_to_bottom = true;
        }

        // Status line.
        let query = self.history_search.trim().to_ascii_lowercase();
        let has_search = !query.is_empty();
        let scope = self.history_search_scope;
        let sender_filter = self.history_sender_filter.clone();
        let filters = self.filters.clone();
        let filtering = has_search || sender_filter.is_some();

        // A history entry passes the current filters.
        let passes = |e: &HistoryEntry| -> bool {
            if !filters.should_show(e.line.chat_type) {
                return false;
            }
            if let Some(sf) = &sender_filter {
                if !e.line.matches_participant(sf) {
                    return false;
                }
            }
            if has_search && !e.line.matches_search(&query, scope) {
                return false;
            }
            true
        };

        // Count matches up front so we can show "N of M" after filtering.
        let matched = self.history_entries.iter().filter(|e| passes(e)).count();
        // "Filtering" for status purposes also covers active type filters.
        let filtering = filtering || matched != self.history_entries.len();

        if self.history_truncated || self.history_error.is_some() {
            ui.horizontal(|ui| {
                if self.history_truncated {
                    ui.label(
                        RichText::new(format!("Capped at {HISTORY_MAX_ENTRIES} entries"))
                            .small()
                            .color(Color32::from_rgb(220, 180, 80)),
                    );
                }
                if let Some(err) = &self.history_error {
                    ui.label(
                        RichText::new(err)
                            .small()
                            .color(Color32::from_rgb(220, 120, 120)),
                    );
                }
            });
        }

        // Discoverability hint mirroring the live chat affordance.
        if self.history_jump_to.is_some() {
            ui.label(
                RichText::new(
                    "Showing the selected message in context · change the search or range to exit",
                )
                .small()
                .color(Color32::DARK_GRAY),
            );
        } else if filtering {
            ui.label(
                RichText::new(
                    "Click a name to filter by sender · right-click a result → Find in chat",
                )
                .small()
                .color(Color32::DARK_GRAY),
            );
        }
        self.show_status_line(ui);

        // Show the date next to each row when the range can span multiple days.
        let show_dates = !matches!(sel, LoadedSel::Range(start, end) if start == end);

        // When a "Find in chat" jump is active, render only a window of entries
        // around the target so the jumped-to line stays in view without rendering
        // the entire (possibly huge) history.
        let jump_window = self.history_jump_to.map(|t| {
            (
                t.saturating_sub(HISTORY_JUMP_WINDOW),
                t.saturating_add(HISTORY_JUMP_WINDOW),
            )
        });
        let jump_target = self.history_jump_to;
        let jump_flash = self.history_jump_flash;
        let do_scroll = self.history_jump_scroll;
        let now = Instant::now();

        let mut pending_sender: Option<String> = None;
        let mut pending_jump: Option<usize> = None;
        let mut pending_status: Option<String> = None;
        let mut jump_scrolled = false;

        // Build a filtered index of entry indices that pass the current filters.
        // This allows virtualized rendering via show_rows without iterating all
        // entries each frame.
        let visible_indices: Vec<usize> = self
            .history_entries
            .iter()
            .enumerate()
            .filter(|(idx, entry)| {
                if let Some((lo, hi)) = jump_window {
                    if *idx < lo || *idx > hi {
                        return false;
                    }
                }
                passes(entry)
            })
            .map(|(idx, _)| idx)
            .collect();

        let total_rows = visible_indices.len();
        let row_height = ui.text_style_height(&egui::TextStyle::Body);
        let spacing = ui.spacing().item_spacing.y;

        // For jump-to-message, find the visual row index and pre-set the scroll
        // offset so the target is in the rendered range (show_rows only renders
        // visible rows, so scroll_to_me alone won't work if the target is off-screen).
        let mut scroll_area = ScrollArea::vertical()
            .id_salt("chat_history_scroll")
            .auto_shrink([false, false]);

        let nav = super::scroll_nav::ScrollNav::read(ui, "chat_history_scroll");

        if do_scroll {
            if let Some(target_idx) = jump_target {
                if let Some(visual_row) = visible_indices.iter().position(|&i| i == target_idx) {
                    let row_pitch = row_height + spacing;
                    let target_offset = visual_row as f32 * row_pitch;
                    let viewport_h = ui.available_height();
                    let offset = (target_offset - viewport_h / 2.0).max(0.0);
                    scroll_area = scroll_area.vertical_scroll_offset(offset);
                }
            }
        } else if self.history_scroll_to_bottom {
            // Pin to the newest messages after a (re)load. A very large offset is
            // clamped by egui to the bottom of the content.
            let row_pitch = row_height + spacing;
            scroll_area = scroll_area.vertical_scroll_offset(total_rows as f32 * row_pitch);
        } else {
            // Keyboard PgUp/PgDn/Home/End navigation.
            scroll_area = nav.apply(scroll_area);
        }
        self.history_scroll_to_bottom = false;

        let scroll_output = scroll_area.show_rows(ui, row_height, total_rows, |ui, row_range| {
            for visible_row in row_range {
                let idx = visible_indices[visible_row];
                let entry = &self.history_entries[idx];

                let flashing = jump_flash.is_some_and(|(s, deadline)| s == idx && now < deadline);
                let bg_idx = if flashing {
                    Some(ui.painter().add(egui::Shape::Noop))
                } else {
                    None
                };

                let date = show_dates.then_some(entry.date);
                let inner = ui.horizontal(|ui| {
                    Self::render_row(ui, &entry.line, date, &query, scope, sprite_renderer)
                });
                let name_rect = inner.inner;

                let mut row_rect = inner.response.rect;
                row_rect.set_right(ui.max_rect().right());
                let row_id = ui.make_persistent_id(("chat_history_row", idx));
                let row_resp = ui.interact(row_rect, row_id, Sense::click());

                if let Some(bi) = bg_idx {
                    ui.painter().set(
                        bi,
                        egui::Shape::rect_filled(row_rect.expand(2.0), 3.0, JUMP_FLASH),
                    );
                }

                if do_scroll && Some(idx) == jump_target {
                    jump_scrolled = true;
                }

                if row_resp.hovered() {
                    if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                        if name_rect.contains(pos) {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                    }
                }

                if row_resp.clicked() {
                    let pos = row_resp
                        .interact_pointer_pos()
                        .or_else(|| ui.input(|i| i.pointer.interact_pos()));
                    if let Some(pos) = pos {
                        if name_rect.contains(pos) {
                            pending_sender = Some(entry.line.displayed_name().to_string());
                        }
                    }
                }

                let name = entry.line.displayed_name().to_string();
                let text = emotes::strip_tags(&entry.line.text);
                row_resp.context_menu(|ui| {
                    if shadcn.btn(ui, "🔍 Find in chat").clicked() {
                        pending_jump = Some(idx);
                        ui.close();
                    }
                    if shadcn.btn(ui, format!("Filter by {name}")).clicked() {
                        pending_sender = Some(name.clone());
                        ui.close();
                    }
                    if shadcn.btn(ui, "📋 Copy IGN").clicked() {
                        ui.ctx().copy_text(name.clone());
                        pending_status = Some(format!("Copied: {name}"));
                        ui.close();
                    }
                    if shadcn.btn(ui, "📋 Copy message").clicked() {
                        ui.ctx().copy_text(text.clone());
                        pending_status = Some(format!("Copied: {text}"));
                        ui.close();
                    }
                });
            }

            if total_rows == 0 {
                if self.history_entries.is_empty() {
                    crate::panels::empty_state(ui, "No chat history for this range", None);
                } else {
                    crate::panels::empty_state(ui, "No history matches your filters", None);
                }
            }
        });
        // Remember the scroll offset for the next PgUp/PgDn keyboard step.
        nav.store(ui, &scroll_output);

        if let Some(name) = pending_sender {
            self.history_sender_filter = Some(name);
            self.history_loaded = None;
            self.clear_history_jump();
        }
        if let Some(status) = pending_status {
            self.status_message = Some((status, Instant::now()));
            ui.ctx().request_repaint();
        }
        if let Some(idx) = pending_jump {
            self.history_search.clear();
            let had_sender = self.history_sender_filter.is_some();
            self.history_sender_filter = None;
            if had_sender {
                // Reload is needed; find the target entry's identity so we can
                // locate it after reload with the new (unfiltered) dataset.
                let target_entry = self.history_entries.get(idx).cloned();
                self.history_loaded = None;
                let sel = self.current_selection();
                self.load_history(sel);
                self.history_loaded = Some(sel);
                // Re-find the entry by matching date + time + sender + text.
                let resolved_idx = target_entry.and_then(|te| {
                    self.history_entries.iter().position(|e| {
                        e.date == te.date
                            && e.line.time == te.line.time
                            && e.line.sender == te.line.sender
                            && e.line.text == te.line.text
                    })
                });
                if let Some(new_idx) = resolved_idx {
                    self.history_jump_to = Some(new_idx);
                    self.history_jump_scroll = true;
                    self.history_jump_flash = Some((new_idx, Instant::now() + JUMP_FLASH_DURATION));
                }
            } else {
                self.history_jump_to = Some(idx);
                self.history_jump_scroll = true;
                self.history_jump_flash = Some((idx, Instant::now() + JUMP_FLASH_DURATION));
            }
        } else if jump_scrolled {
            // Scrolled to the target this frame; stop forcing scroll so the user
            // can move freely while the context window stays put.
            self.history_jump_scroll = false;
        }

        // Keep repainting while the flash is active, then drop it.
        if let Some((_, deadline)) = self.history_jump_flash {
            if Instant::now() < deadline {
                ui.ctx().request_repaint();
            } else {
                self.history_jump_flash = None;
            }
        }
    }
}

impl Panel for ChatPanel {
    fn show(&mut self, ui: &mut egui::Ui, ctx: &mut PanelContext) -> Vec<AppAction> {
        if self.render(ui, ctx.shadcn, ctx.sprite_renderer) {
            vec![AppAction::SaveChatSettings]
        } else {
            vec![]
        }
    }

    fn handle_event(
        &mut self,
        event: &realmhound_core::GameEvent,
        session: &realmhound_core::GameSession,
    ) -> Vec<AppAction> {
        if let realmhound_core::GameEvent::TextReceived(ref text) = event {
            if let Some(mut chat_msg) = ChatMessage::from_text_packet(text) {
                let mut actions = Vec::new();
                if let Some(boss_id) = chat_msg.boss_id {
                    // Encounter callouts are surfaced in the Live Feed only, not the chat list.
                    actions.push(AppAction::PushBossCall {
                        boss_id,
                        text: chat_msg.text.clone(),
                    });
                    return actions;
                }
                // Whispers always involve the local player. Both directions render
                // the full "sender → recipient: text"; we only tag outgoing whispers
                // (sender == me) so click-to-filter targets the conversation partner.
                if chat_msg.chat_type == ChatType::Whisper {
                    if let Some(me) = session.connection.detected_account_name.as_deref() {
                        if chat_msg.sender.eq_ignore_ascii_case(me) {
                            chat_msg.is_outgoing = true;
                        }
                    }
                }
                self.add_message(chat_msg);
                return actions;
            }
        }
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use realmhound_core::protocol::TextPacket;

    fn make_packet(name: &str, recipient: &str, text: &str) -> TextPacket {
        TextPacket {
            name: name.to_string(),
            object_id: 100,
            num_stars: 0,
            bubble_time: 0,
            recipient: recipient.to_string(),
            text: text.to_string(),
            clean_text: text.to_string(),
            is_supporter: false,
            star_background: 0,
        }
    }

    #[test]
    fn chat_logs_to_the_injected_directory() {
        let temp = tempfile::tempdir().unwrap();
        let log_dir = temp.path().join("logs").join("chat");
        let settings = realmhound_core::settings::ChatSettings::default();
        let mut panel = ChatPanel::new_with_settings(&settings, log_dir.clone());

        let message = ChatMessage {
            timestamp: Local::now(),
            sender: "Pillar".to_string(),
            text: "hello world".to_string(),
            chat_type: ChatType::Normal,
            recipient: None,
            stars: 0,
            is_supporter: false,
            boss_id: None,
            is_outgoing: false,
        };
        panel.add_message(message);

        // A daily log file was created under the injected per-account directory.
        assert!(log_dir.exists());
        let count = std::fs::read_dir(&log_dir).unwrap().count();
        assert_eq!(
            count, 1,
            "one daily chat log written under the injected dir"
        );
    }

    #[test]
    fn chat_without_bound_directory_writes_nothing() {
        let settings = realmhound_core::settings::ChatSettings::default();
        let mut panel = ChatPanel::new_with_settings(&settings, std::path::PathBuf::new());
        // Must not panic or create files when no log directory is bound.
        panel.add_message(ChatMessage {
            timestamp: Local::now(),
            sender: "Pillar".to_string(),
            text: "hello".to_string(),
            chat_type: ChatType::Normal,
            recipient: None,
            stars: 0,
            is_supporter: false,
            boss_id: None,
            is_outgoing: false,
        });
    }

    #[test]
    fn guild_chat_classified_as_guild_not_whisper() {
        let packet = make_packet("Pillar", "*Guild*", "hello guild");
        let msg = ChatMessage::from_text_packet(&packet).expect("guild message should be kept");
        assert_eq!(msg.chat_type, ChatType::Guild);
        // Guild messages should not carry a recipient (render green "Sender: text").
        assert_eq!(msg.recipient, None);
    }

    #[test]
    fn whisper_still_classified_with_recipient() {
        let packet = make_packet("Lovens", "Pillar", "secret");
        let msg = ChatMessage::from_text_packet(&packet).expect("whisper should be kept");
        assert_eq!(msg.chat_type, ChatType::Whisper);
        assert_eq!(msg.recipient.as_deref(), Some("Pillar"));
    }

    #[test]
    fn alien_wave_override_maps_calbrik_taunts_to_waves() {
        let cases = [
            ("Atmosphere breach imminent, prepare all units!", "Alien Invasion Adept - Wave 1", "AI Alien Soldier"),
            ("Inhabitants have proven capable of retaliation! Send out the extermination units!", "Alien Invasion Adept - Wave 2", "AI Alien Mini Tank"),
            ("Surrender now or be faced with extreme prejudice!", "Alien Invasion Adept - Wave 3", "AI Alien Tank"),
            ("Deploy punisher units for full elimination of life!", "Alien Invasion Adept - Wave 4", "AI Alien Pod"),
            ("You will regret having made a fool of me! The members of the Calbrik Guard will grant you a slow and agonizing death!", "Alien Invasion Veteran - Wave 1", "AI Alien Sage"),
            ("Release the Mech Squad! With their powers our victory will be definitive!", "Alien Invasion Veteran - Wave 2", "AI Alien Buster"),
            ("Throw everything we have at them, even those creepy pods! We cannot lose this war!", "Alien Invasion Veteran - Wave 3", "AI Alien Destroyer"),
            ("How can eliminating such simple humanoid creatures be this difficult?!", "Alien Invasion Veteran - Wave 4", "AI Alien Spoder"),
        ];
        for (text, name, minion) in cases {
            assert_eq!(
                ChatMessage::alien_wave_override(text),
                Some((name, minion)),
                "{text}"
            );
        }
    }

    #[test]
    fn alien_wave_override_ignores_unrelated_text() {
        assert_eq!(
            ChatMessage::alien_wave_override("Fine, I'll do it myself..."),
            None
        );
        assert_eq!(ChatMessage::alien_wave_override("hello guild"), None);
    }

    #[test]
    fn search_is_case_insensitive_over_sender_and_text() {
        let packet = make_packet("EangoNPE", "", "Good luck out there");
        let msg = ChatMessage::from_text_packet(&packet).unwrap();
        assert!(msg.matches_search("eango", SearchScope::All));
        assert!(msg.matches_search("luck", SearchScope::All));
        assert!(!msg.matches_search("missing", SearchScope::All));
    }

    #[test]
    fn name_only_search_ignores_message_text() {
        let packet = make_packet("EangoNPE", "", "Good luck out there");
        let msg = ChatMessage::from_text_packet(&packet).unwrap();
        // Name still matches in name-only mode...
        assert!(msg.matches_search("eango", SearchScope::Name));
        // ...but message text does not.
        assert!(!msg.matches_search("luck", SearchScope::Name));
    }

    #[test]
    fn text_only_search_ignores_player_name() {
        let packet = make_packet("EangoNPE", "", "Good luck out there");
        let msg = ChatMessage::from_text_packet(&packet).unwrap();
        // Message text matches in text-only mode...
        assert!(msg.matches_search("luck", SearchScope::Text));
        // ...but the player name does not.
        assert!(!msg.matches_search("eango", SearchScope::Text));
    }

    #[test]
    fn displayed_name_uses_recipient_for_outgoing_whisper() {
        let packet = make_packet("Pillar", "Lovens", "shatts when?");
        let mut msg = ChatMessage::from_text_packet(&packet).unwrap();
        // Simulate handle_event marking this as the local player's outgoing whisper.
        msg.is_outgoing = true;
        // displayed_name (the conversation partner) drives click-to-filter.
        assert_eq!(msg.displayed_name(), "Lovens");
        // Both participant names are searchable now that both are rendered.
        assert!(msg.matches_search("lovens", SearchScope::Name));
        assert!(msg.matches_search("pillar", SearchScope::Name));
        // ...but message text is not, in name scope.
        assert!(!msg.matches_search("shatts", SearchScope::Name));
    }

    #[test]
    fn highlight_job_splits_around_match() {
        // ASCII lowercasing keeps byte offsets aligned with the original text.
        let job = ChatPanel::highlight_job("Hello World", "world", Color32::WHITE);
        let combined: String = job
            .sections
            .iter()
            .map(|s| job.text[s.byte_range.clone()].to_string())
            .collect();
        assert_eq!(combined, "Hello World");
        // The matched section preserves the original casing.
        assert!(job
            .sections
            .iter()
            .any(|s| &job.text[s.byte_range.clone()] == "World"));
    }

    #[test]
    fn parse_public_guild_party_lines() {
        let public = parse_log_line("12:34:56 [PUBLIC] Pillar: hello world").unwrap();
        assert_eq!(public.chat_type, ChatType::Normal);
        assert_eq!(public.time, "12:34:56");
        assert_eq!(public.sender, "Pillar");
        assert_eq!(public.recipient, None);
        assert_eq!(public.text, "hello world");

        let guild = parse_log_line("00:00:01 [GUILD] Lovens: gg").unwrap();
        assert_eq!(guild.chat_type, ChatType::Guild);
        assert_eq!(guild.sender, "Lovens");
        assert_eq!(guild.text, "gg");

        let party = parse_log_line("23:59:59 [PARTY] Eango: pull").unwrap();
        assert_eq!(party.chat_type, ChatType::Party);
        assert_eq!(party.sender, "Eango");
        assert_eq!(party.text, "pull");
    }

    #[test]
    fn parse_whisper_line() {
        let whisper = parse_log_line("08:15:00 [WHISPER] Pillar → Lovens: shatts when?").unwrap();
        assert_eq!(whisper.chat_type, ChatType::Whisper);
        assert_eq!(whisper.sender, "Pillar");
        assert_eq!(whisper.recipient.as_deref(), Some("Lovens"));
        assert_eq!(whisper.text, "shatts when?");
    }

    #[test]
    fn parse_legacy_whisper_without_recipient() {
        // Older logs (and incoming whispers) recorded "[WHISPER] sender: text"
        // with no recipient. They should still parse, with recipient = None.
        let whisper = parse_log_line("08:15:00 [WHISPER] Lovens: use → key").unwrap();
        assert_eq!(whisper.chat_type, ChatType::Whisper);
        assert_eq!(whisper.sender, "Lovens");
        assert_eq!(whisper.recipient, None);
        // An arrow inside the message body must not be mistaken for a recipient.
        assert_eq!(whisper.text, "use → key");
    }

    #[test]
    fn parse_handles_colon_and_arrow_inside_text() {
        // First " → " separates names; first ": " ends the recipient. Later
        // occurrences inside the message body must be preserved.
        let whisper =
            parse_log_line("08:15:00 [WHISPER] Pillar → Lovens: hello → world: still text")
                .unwrap();
        assert_eq!(whisper.sender, "Pillar");
        assert_eq!(whisper.recipient.as_deref(), Some("Lovens"));
        assert_eq!(whisper.text, "hello → world: still text");

        let public = parse_log_line("08:15:00 [PUBLIC] Pillar: ratio: 3:1 here").unwrap();
        assert_eq!(public.sender, "Pillar");
        assert_eq!(public.text, "ratio: 3:1 here");
    }

    #[test]
    fn parse_allows_empty_message_text() {
        let public = parse_log_line("08:15:00 [PUBLIC] Pillar: ").unwrap();
        assert_eq!(public.sender, "Pillar");
        assert_eq!(public.text, "");
    }

    #[test]
    fn parse_rejects_malformed_lines() {
        // Empty / whitespace only.
        assert!(parse_log_line("").is_none());
        assert!(parse_log_line("   ").is_none());
        // Invalid time.
        assert!(parse_log_line("99:99:99 [PUBLIC] Pillar: hi").is_none());
        assert!(parse_log_line("notime [PUBLIC] Pillar: hi").is_none());
        // Unknown / missing type tag.
        assert!(parse_log_line("12:00:00 [ENEMY] Oryx: die").is_none());
        assert!(parse_log_line("12:00:00 Pillar: no brackets").is_none());
        // Missing the sender/text separator.
        assert!(parse_log_line("12:00:00 [PUBLIC] Pillar no colon").is_none());
        // Empty sender / recipient.
        assert!(parse_log_line("12:00:00 [PUBLIC] : hi").is_none());
        assert!(parse_log_line("12:00:00 [WHISPER] Pillar → : hi").is_none());
        assert!(parse_log_line("12:00:00 [WHISPER]  → Lovens: hi").is_none());
    }

    #[test]
    fn parse_tolerates_trailing_newline() {
        let public = parse_log_line("12:34:56 [PUBLIC] Pillar: hello\r\n").unwrap();
        assert_eq!(public.text, "hello");
    }

    #[test]
    fn parse_reads_star_and_supporter_metadata() {
        // Star rank only.
        let public = parse_log_line("12:00:00 [PUBLIC] ★64 Pillar: hi").unwrap();
        assert_eq!(public.sender, "Pillar");
        assert_eq!(public.text, "hi");
        assert_eq!(public.stars, 64);
        assert!(!public.is_supporter);

        // Star rank and supporter badge on a whisper.
        let whisper = parse_log_line("12:00:00 [WHISPER] ★80 ♦ Pillar → Lovens: yo").unwrap();
        assert_eq!(whisper.sender, "Pillar");
        assert_eq!(whisper.recipient.as_deref(), Some("Lovens"));
        assert_eq!(whisper.text, "yo");
        assert_eq!(whisper.stars, 80);
        assert!(whisper.is_supporter);
    }

    #[test]
    fn parse_tolerates_lines_without_metadata() {
        // Older logs have no star/supporter prefix; stars default to 0.
        let public = parse_log_line("12:00:00 [PUBLIC] Pillar: hi").unwrap();
        assert_eq!(public.stars, 0);
        assert!(!public.is_supporter);
        // A message body may itself contain a star glyph without being metadata.
        let star_text = parse_log_line("12:00:00 [PUBLIC] Pillar: ★ nice").unwrap();
        assert_eq!(star_text.sender, "Pillar");
        assert_eq!(star_text.text, "★ nice");
        assert_eq!(star_text.stars, 0);
    }

    #[test]
    fn history_matches_search_name_and_text() {
        let line = ParsedLogLine {
            time: "12:00:00".to_string(),
            chat_type: ChatType::Whisper,
            sender: "Pillar".to_string(),
            recipient: Some("Lovens".to_string()),
            text: "see you there".to_string(),
            stars: 0,
            is_supporter: false,
        };
        // Sender and recipient both matchable in name-only mode.
        assert!(line.matches_search("pillar", SearchScope::Name));
        assert!(line.matches_search("lovens", SearchScope::Name));
        // Text matches only when not name-only.
        assert!(line.matches_search("there", SearchScope::All));
        assert!(!line.matches_search("there", SearchScope::Name));
        assert!(!line.matches_search("missing", SearchScope::All));
    }

    #[test]
    fn encounter_overrides_cover_known_bad_names() {
        // Keys are the raw `stringlist.<key>.new.N` segments captured
        // from live packets. Asserts the curated display name + sprite id_name.
        assert_eq!(
            ChatMessage::encounter_name_override("Lord_of_the_Lost_Lands"),
            Some(("Lord of the Lost Lands", "Lord of the Lost Lands"))
        );
        assert_eq!(
            ChatMessage::encounter_name_override("Flying_Behemoth_Egg"),
            Some(("Flying Behemoth", "Flying Behemoth"))
        );
        assert_eq!(
            ChatMessage::encounter_name_override("Temple_Encounter"),
            Some(("Jade and Garnet Statues", "Jade Statue"))
        );
        assert_eq!(
            ChatMessage::encounter_name_override("Beer_Encounter_Spawner"),
            Some(("Beer God", "Beer God"))
        );
        assert_eq!(
            ChatMessage::encounter_name_override("EH_Event_Taunt_Controller"),
            Some(("Killer Bee Nest", "EH Queen Blue Hive"))
        );
    }

    #[test]
    fn encounter_override_absent_for_normal_boss() {
        // Non-overridden bosses fall through to the default id_name resolution.
        assert_eq!(
            ChatMessage::encounter_name_override("The_Plague_Doctor"),
            None
        );
    }

    #[test]
    fn alien_taunt_is_suppressed() {
        // AI_Alien_Taunt is Oryx's pre-event taunt, not the real
        // UFO spawn. The actual callout comes from Commander Calbrik.
        assert!(ChatMessage::is_suppressed_encounter("AI_Alien_Taunt"));
        assert!(!ChatMessage::is_suppressed_encounter("The_Plague_Doctor"));
    }
}

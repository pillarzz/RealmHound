//! MapInfo packet implementation.
//!
//! Received in response to HelloPacket when entering a new map/area.
//! Contains map dimensions, name, display name, and various settings.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// MapInfo packet (ID 92) - Incoming
///
/// Sent by server when player enters a new map. Contains the map's display name
/// which is used to detect special areas like Pet Yard and Daily Quest Room.
#[derive(Debug, Clone)]
pub struct MapInfoPacket {
    /// Map width in tiles
    pub width: i32,
    /// Map height in tiles
    pub height: i32,
    /// Internal map name
    pub name: String,
    /// Display name shown to player (e.g., "Daily Quest Room", "Pet Yard")
    pub display_name: String,
    /// Realm name (if in a realm)
    pub realm_name: String,
    /// Fame points or seed value
    pub fp: i32,
    /// Background type
    pub background: i32,
    /// Difficulty level
    pub difficulty: f32,
    /// Whether player teleport is allowed
    pub allow_player_teleport: bool,
    /// Whether this is a no-save area
    pub no_save: bool,
    /// Whether to show displays
    pub show_displays: bool,
    /// Maximum player count
    pub max_player_count: i16,
    /// Time when game was opened
    pub game_opened_time: i32,
    /// Version number string
    pub version_number: String,
    /// View distance
    pub view_distance: i16,
    /// Dungeon modifiers string (raw, may contain multiple separated by ';')
    pub dungeon_modifiers: String,
    /// Background color
    pub bg_color: i16,
    /// Maximum realm score (only present in realms, -1 if not set)
    pub max_realm_score: i32,
    /// Current realm score (only present in realms, -1 if not set)
    pub current_realm_score: i32,
}

impl RotmgPacket for MapInfoPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let width = reader.read_i32()?;
        let height = reader.read_i32()?;
        let name = reader.read_string()?;
        let display_name = reader.read_string()?;
        let realm_name = reader.read_string()?;
        let fp = reader.read_i32()?;
        let background = reader.read_i32()?;
        let difficulty = reader.read_f32()?;
        let allow_player_teleport = reader.read_bool()?;
        let no_save = reader.read_bool()?;
        let show_displays = reader.read_bool()?;
        let max_player_count = reader.read_i16()?;
        let game_opened_time = reader.read_i32()?;
        let version_number = reader.read_string()?;
        let _unknown1 = reader.read_i16()?;
        let view_distance = reader.read_i16()?;
        let _unknown2 = reader.read_bool()?;
        let _unknown_int = reader.read_i32()?;
        let dungeon_modifiers = reader.read_string()?;
        let bg_color = reader.read_i16()?;

        // Realm score is optional - only present when remaining >= 8 bytes
        let (max_realm_score, current_realm_score) = if reader.remaining() >= 8 {
            (reader.read_i32()?, reader.read_i32()?)
        } else {
            (-1, -1)
        };

        Ok(Self {
            width,
            height,
            name,
            display_name,
            realm_name,
            fp,
            background,
            difficulty,
            allow_player_teleport,
            no_save,
            show_displays,
            max_player_count,
            game_opened_time,
            version_number,
            view_distance,
            dungeon_modifiers,
            bg_color,
            max_realm_score,
            current_realm_score,
        })
    }

    fn description(&self) -> String {
        format!(
            "MapInfo: {} ({}x{})",
            self.display_name, self.width, self.height
        )
    }
}

impl MapInfoPacket {
    /// Check if this map is the Pet Yard.
    pub fn is_pet_yard(&self) -> bool {
        self.display_name == "Pet Yard"
    }

    /// Check if this map is the Daily Quest Room.
    pub fn is_daily_quest_room(&self) -> bool {
        self.display_name == "Daily Quest Room"
    }

    /// Check if this map allows character list API calls.
    /// Only Pet Yard and Daily Quest Room allow this.
    pub fn allows_char_list_api(&self) -> bool {
        self.is_pet_yard() || self.is_daily_quest_room()
    }

    /// Decode the raw `dungeon_modifiers` string into modifier id tokens and an
    /// optional dungeon grade.
    ///
    /// Wire format: modifier ids separated by
    /// `;`, with an optional grade appended to the final token via `|`. The
    /// grade may also appear as a standalone token (e.g. `";|S"`).
    ///
    /// Examples:
    /// - `"CHEF;GENEROUS|S"` -> (`["CHEF", "GENEROUS"]`, `Some("S")`)
    /// - `";|S"` -> (`[]`, `Some("S")`)
    /// - `"CHEF;"` -> (`["CHEF"]`, `None`)
    /// - `""` -> (`[]`, `None`)
    pub fn decode_dungeon_modifiers(&self) -> (Vec<String>, Option<String>) {
        let raw = self.dungeon_modifiers.trim();
        if raw.is_empty() {
            return (Vec::new(), None);
        }

        let mut names = Vec::new();
        let mut grade = None;

        for part in raw.split(';') {
            let (name, part_grade) = match part.split_once('|') {
                Some((name, g)) => (name, Some(g)),
                None => (part, None),
            };

            let name = name.trim();
            if !name.is_empty() {
                names.push(name.to_string());
            }

            if let Some(g) = part_grade {
                let g = g.trim();
                if !g.is_empty() {
                    grade = Some(g.to_string());
                }
            }
        }

        (names, grade)
    }

    /// Whether this map is a hub or realm rather than a dungeon.
    ///
    /// Used to suppress dungeon-entry feed output for non-dungeon maps. This is
    /// a display heuristic, not protocol truth: it relies on a known set of
    /// special locations plus the realm score signal.
    pub fn is_hub_or_realm(&self) -> bool {
        // Realm score is only present (> 0) when inside a realm.
        if self.max_realm_score > 0 {
            return true;
        }

        let name = self.display_name.trim().to_lowercase();
        if matches!(
            name.as_str(),
            "" | "nexus"
                | "{s.nexus}"
                | "vault"
                | "{s.vault}"
                | "pet yard"
                | "{s.petyard}"
                | "daily quest room"
                | "guild hall"
                | "{s.guild}"
                | "realm of the mad god"
                | "{s.rotmg}"
                | "bazaar"
                | "cloth bazaar"
                | "grand bazaar"
                | "marketplace"
                | "tutorial"
                | "{s.tutorial}"
        ) {
            return true;
        }

        self.is_non_joinable_special()
    }

    /// Whether this map is a non-joinable special area (Oryx's Court and the
    /// guild hall). These show up as "dungeons" but cannot be joined by others,
    /// so they must not produce a dungeon-entry feed item.
    ///
    /// Matching is resilient to localization-key (`{s.oryx_s_castle}`) vs plain
    /// (`Oryx's Castle`) forms by reducing both to lowercase ASCII alphanumerics.
    pub fn is_non_joinable_special(&self) -> bool {
        let raw = self.display_name.trim();
        let inner = raw
            .strip_prefix("{s.")
            .and_then(|s| s.strip_suffix('}'))
            .unwrap_or(raw);
        let canon: String = inner
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .flat_map(|c| c.to_lowercase())
            .collect();
        matches!(
            canon.as_str(),
            // With and without the possessive "s" to tolerate either
            // localization-key spelling ({s.oryx_s_castle} vs {s.oryx_castle}).
            "oryxscastle"
                | "oryxcastle"
                | "oryxschamber"
                | "oryxchamber"
                | "oryxssanctuary"
                | "oryxsanctuary"
                | "courtoforyx"
                | "winecellar"
                | "guildhall"
        )
    }

    /// Whether this map should produce a dungeon-entry feed item.
    ///
    /// True for real dungeons: a non-empty display name that is not a known hub
    /// or realm. This is a display heuristic and may include the occasional
    /// non-dungeon special area.
    pub fn is_dungeon_for_feed(&self) -> bool {
        !self.display_name.trim().is_empty() && !self.is_hub_or_realm()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_map_info_bytes(display_name: &str) -> Vec<u8> {
        let mut data = Vec::new();

        // width: i32
        data.extend_from_slice(&100i32.to_be_bytes());
        // height: i32
        data.extend_from_slice(&100i32.to_be_bytes());
        // name: string
        let name = "vault";
        data.extend_from_slice(&(name.len() as u16).to_be_bytes());
        data.extend_from_slice(name.as_bytes());
        // display_name: string
        data.extend_from_slice(&(display_name.len() as u16).to_be_bytes());
        data.extend_from_slice(display_name.as_bytes());
        // realm_name: string (empty)
        data.extend_from_slice(&0u16.to_be_bytes());
        // fp: i32
        data.extend_from_slice(&12345i32.to_be_bytes());
        // background: i32
        data.extend_from_slice(&0i32.to_be_bytes());
        // difficulty: f32
        data.extend_from_slice(&1.0f32.to_be_bytes());
        // allow_player_teleport: bool
        data.push(1);
        // no_save: bool
        data.push(0);
        // show_displays: bool
        data.push(1);
        // max_player_count: i16
        data.extend_from_slice(&25i16.to_be_bytes());
        // game_opened_time: i32
        data.extend_from_slice(&0i32.to_be_bytes());
        // version_number: string
        let version = "6.5.0";
        data.extend_from_slice(&(version.len() as u16).to_be_bytes());
        data.extend_from_slice(version.as_bytes());
        // unknown1: i16
        data.extend_from_slice(&0i16.to_be_bytes());
        // view_distance: i16
        data.extend_from_slice(&15i16.to_be_bytes());
        // unknown2: bool
        data.push(0);
        // unknown_int: i32
        data.extend_from_slice(&0i32.to_be_bytes());
        // dungeon_modifiers: string (empty)
        data.extend_from_slice(&0u16.to_be_bytes());
        // bg_color: i16
        data.extend_from_slice(&0i16.to_be_bytes());
        // max_realm_score: i32 (optional)
        data.extend_from_slice(&(-1i32).to_be_bytes());
        // current_realm_score: i32 (optional)
        data.extend_from_slice(&(-1i32).to_be_bytes());

        data
    }

    #[test]
    fn test_map_info_deserialize() {
        let data = build_map_info_bytes("Nexus");
        let mut reader = PacketReader::new(&data);
        let packet = MapInfoPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.width, 100);
        assert_eq!(packet.height, 100);
        assert_eq!(packet.name, "vault");
        assert_eq!(packet.display_name, "Nexus");
        assert!(!packet.is_pet_yard());
        assert!(!packet.is_daily_quest_room());
        assert!(!packet.allows_char_list_api());
    }

    #[test]
    fn test_map_info_pet_yard() {
        let data = build_map_info_bytes("Pet Yard");
        let mut reader = PacketReader::new(&data);
        let packet = MapInfoPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.display_name, "Pet Yard");
        assert!(packet.is_pet_yard());
        assert!(!packet.is_daily_quest_room());
        assert!(packet.allows_char_list_api());
    }

    #[test]
    fn test_map_info_daily_quest_room() {
        let data = build_map_info_bytes("Daily Quest Room");
        let mut reader = PacketReader::new(&data);
        let packet = MapInfoPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.display_name, "Daily Quest Room");
        assert!(!packet.is_pet_yard());
        assert!(packet.is_daily_quest_room());
        assert!(packet.allows_char_list_api());
    }

    #[test]
    fn test_map_info_description() {
        let data = build_map_info_bytes("The Shatters");
        let mut reader = PacketReader::new(&data);
        let packet = MapInfoPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.description(), "MapInfo: The Shatters (100x100)");
    }

    fn packet_with_modifiers(
        display_name: &str,
        modifiers: &str,
        max_realm_score: i32,
    ) -> MapInfoPacket {
        MapInfoPacket {
            width: 100,
            height: 100,
            name: "map".into(),
            display_name: display_name.into(),
            realm_name: String::new(),
            fp: 0,
            background: 0,
            difficulty: 1.0,
            allow_player_teleport: true,
            no_save: false,
            show_displays: true,
            max_player_count: 25,
            game_opened_time: 0,
            version_number: "6.5.0".into(),
            view_distance: 15,
            dungeon_modifiers: modifiers.into(),
            bg_color: 0,
            max_realm_score,
            current_realm_score: -1,
        }
    }

    #[test]
    fn test_decode_modifiers_with_grade() {
        let p = packet_with_modifiers("Spider Den", "CHEF;GENEROUS|S", -1);
        let (mods, grade) = p.decode_dungeon_modifiers();
        assert_eq!(mods, vec!["CHEF".to_string(), "GENEROUS".to_string()]);
        assert_eq!(grade, Some("S".to_string()));
    }

    #[test]
    fn test_decode_modifiers_standalone_grade() {
        let p = packet_with_modifiers("Spider Den", ";|S", -1);
        let (mods, grade) = p.decode_dungeon_modifiers();
        assert!(mods.is_empty());
        assert_eq!(grade, Some("S".to_string()));
    }

    #[test]
    fn test_decode_modifiers_trailing_separator() {
        let p = packet_with_modifiers("Spider Den", "CHEF;", -1);
        let (mods, grade) = p.decode_dungeon_modifiers();
        assert_eq!(mods, vec!["CHEF".to_string()]);
        assert_eq!(grade, None);
    }

    #[test]
    fn test_decode_modifiers_empty() {
        let p = packet_with_modifiers("Spider Den", "", -1);
        let (mods, grade) = p.decode_dungeon_modifiers();
        assert!(mods.is_empty());
        assert_eq!(grade, None);
    }

    #[test]
    fn test_decode_modifiers_single_with_grade() {
        let p = packet_with_modifiers("Spider Den", "SOUVENIR_1|A", -1);
        let (mods, grade) = p.decode_dungeon_modifiers();
        assert_eq!(mods, vec!["SOUVENIR_1".to_string()]);
        assert_eq!(grade, Some("A".to_string()));
    }

    #[test]
    fn test_is_dungeon_for_feed() {
        assert!(packet_with_modifiers("Spider Den", "", -1).is_dungeon_for_feed());
        assert!(packet_with_modifiers("The Shatters", "CHEF", -1).is_dungeon_for_feed());

        // Hubs / realm are not dungeons.
        assert!(!packet_with_modifiers("Nexus", "", -1).is_dungeon_for_feed());
        assert!(!packet_with_modifiers("{s.vault}", "", -1).is_dungeon_for_feed());
        assert!(!packet_with_modifiers("Pet Yard", "", -1).is_dungeon_for_feed());
        assert!(!packet_with_modifiers("Daily Quest Room", "", -1).is_dungeon_for_feed());
        assert!(!packet_with_modifiers("", "", -1).is_dungeon_for_feed());
        // Realm detected by score even with an unusual name.
        assert!(!packet_with_modifiers("Realm of the Mad God", "", 1000).is_dungeon_for_feed());
    }

    #[test]
    fn non_joinable_special_areas_are_not_dungeons() {
        // Oryx's Court and the guild hall are not joinable, in both plain and
        // localization-key forms.
        for name in [
            "Oryx's Castle",
            "{s.oryx_s_castle}",
            "Oryx's Chamber",
            "{s.oryx_s_chamber}",
            "Oryx's Sanctuary",
            "{s.oryx_s_sanctuary}",
            "Wine Cellar",
            "{s.wine_cellar}",
            "Court of Oryx",
            "{s.court_of_oryx}",
            "Guild Hall",
            "{s.guild}",
            // No-possessive localization variants and curly apostrophe.
            "{s.oryx_chamber}",
            "{s.oryx_sanctuary}",
            "Oryxs Castle",
            "Oryx\u{2019}s Chamber",
        ] {
            assert!(
                !packet_with_modifiers(name, "", -1).is_dungeon_for_feed(),
                "{name} should not be a dungeon feed entry"
            );
        }

        // A real dungeon whose name starts with the same letters still counts.
        assert!(packet_with_modifiers("Snake Pit", "", -1).is_dungeon_for_feed());
    }
}

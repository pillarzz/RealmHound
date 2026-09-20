//! Embedded PNG icons compiled into the binary.
//!
//! These are small custom icons (16x16) that ship inside the executable rather
//! than coming from the game's sprite atlases. They are rendered through
//! [`crate::rendering::SpriteRenderer::draw_embedded_icon`].
//!
//! Each variant owns its raw bytes and a stable texture name, so adding a new
//! embedded icon is a single enum addition plus a row in [`EmbeddedIcon::ALL`].

/// Identifier for a custom PNG icon embedded in the binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EmbeddedIcon {
    /// Seasonal character icon (SeasonalChar.png).
    SeasonalChar,
    /// Party icon (Party.png).
    Party,
    /// Dungeon Types Completed stat icon (Dungeon Types Completed.png).
    DungeonTypesCompleted,
    /// Dungeon grade "S" badge (grade_s.png).
    GradeS,
    /// Dungeon grade "A" badge (grade_a.png).
    GradeA,
    /// Dungeon grade "B" badge (grade_b.png).
    GradeB,
    /// Dungeon grade "C" badge (grade_c.png).
    GradeC,
    /// Dungeon grade "D" badge (grade_d.png).
    GradeD,
    /// Dungeon "no grade" badge (grade_no.png).
    GradeNo,
    /// Reward mod-type icon — gift (mods_reward.png).
    ModTypeReward,
    /// Unique mod-type icon — diamond (mods_unique.png).
    ModTypeUnique,
    /// Dungeon-special mod-type icon — spark (mods_dung_unique.png).
    ModTypeSpecial,
    /// Grayscale dust icon for dust-bonus display (grey_dust.png).
    DustGrey,
    /// Realmeye logo, used for the Party panel's Realmeye link button (party/realmeye.png).
    RealmeyeLogo,
    /// Discord server icon, used for the About modal's Discord invite link (discord.png).
    Discord,
    /// Discord brand logo (blurple Clyde mark), used on the About modal's Discord card (discord_logo.png).
    DiscordLogo,
    /// Live Feed tab icon (NexusCrystal.png) - Nexus Crystal item, not in the atlas.
    NexusCrystal,
    /// Bare Common forge material gem (ForgeMaterial-Common.png).
    ForgeCommon,
    /// Bare Rare forge material gem (ForgeMaterial-Rare.png).
    ForgeRare,
    /// Bare Legendary forge material gem (ForgeMaterial-Legendary.png).
    ForgeLegendary,
    /// Bare Mythical forge material gem (ForgeMaterial-Mythical.png).
    ForgeMythical,
    /// Forge Fire blue flame currency icon (ForgeFire.png).
    ForgeFire,
    /// Party leader marker/crown (party/party_lead.png).
    PartyLeader,
    /// Watchlisted player marker/eye (party/party_watch.png).
    PartyWatch,
    /// Banned player marker (party/party_ban.png).
    PartyBan,
    /// Party DM/message button (party/party_message.png).
    PartyMessage,
    /// Party promote button (party/party_promote.png).
    PartyPromote,
    /// Party kick button (party/party_kick.png).
    PartyKick,
    /// Gift box shown on a claimable mission card (gift_small.png).
    Gift,
    /// Purple oryx-head minimap marker for encounter/quest mission objectives
    /// (encounter_oryx.png). No in-game object exists for this sprite.
    EncounterOryx,
    /// Compass icon used for the Live Feed Taskbar track toggle on Mission and
    /// Daily Quest cards (compass.png). No in-game object exists for this sprite.
    Compass,
    /// Battle-pass XP token, shown on the BXP bonus row (bxp.png).
    Bxp,
    /// Legacy Pirate Cave portal (original art missing from game assets).
    LegacyPortalPirateCave,
    /// Legacy Spider Den portal (original art missing from game assets).
    LegacyPortalSpiderDen,
    /// Legacy Sprite World portal (original art missing from game assets).
    LegacyPortalSpriteWorld,
    /// Legacy Undead Lair portal (original art missing from game assets).
    LegacyPortalUndeadLair,
    /// Legacy Abyss of Demons portal (original art missing from game assets).
    LegacyPortalAbyssOfDemons,
    /// Legacy The Crawling Depths portal (original art missing from game assets).
    LegacyPortalCrawlingDepths,
    /// Legacy Woodland Labyrinth portal (original art missing from game assets).
    LegacyPortalWoodlandLabyrinth,
    /// Legacy The Shatters portal (original art missing from game assets).
    LegacyPortalShatters,
}

impl EmbeddedIcon {
    /// Every embedded icon, used to load all textures up front.
    pub const ALL: &'static [EmbeddedIcon] = &[
        EmbeddedIcon::SeasonalChar,
        EmbeddedIcon::Party,
        EmbeddedIcon::DungeonTypesCompleted,
        EmbeddedIcon::GradeS,
        EmbeddedIcon::GradeA,
        EmbeddedIcon::GradeB,
        EmbeddedIcon::GradeC,
        EmbeddedIcon::GradeD,
        EmbeddedIcon::GradeNo,
        EmbeddedIcon::ModTypeReward,
        EmbeddedIcon::ModTypeUnique,
        EmbeddedIcon::ModTypeSpecial,
        EmbeddedIcon::DustGrey,
        EmbeddedIcon::RealmeyeLogo,
        EmbeddedIcon::Discord,
        EmbeddedIcon::DiscordLogo,
        EmbeddedIcon::NexusCrystal,
        EmbeddedIcon::ForgeCommon,
        EmbeddedIcon::ForgeRare,
        EmbeddedIcon::ForgeLegendary,
        EmbeddedIcon::ForgeMythical,
        EmbeddedIcon::ForgeFire,
        EmbeddedIcon::PartyLeader,
        EmbeddedIcon::PartyWatch,
        EmbeddedIcon::PartyBan,
        EmbeddedIcon::PartyMessage,
        EmbeddedIcon::PartyPromote,
        EmbeddedIcon::PartyKick,
        EmbeddedIcon::Gift,
        EmbeddedIcon::EncounterOryx,
        EmbeddedIcon::Compass,
        EmbeddedIcon::Bxp,
        EmbeddedIcon::LegacyPortalPirateCave,
        EmbeddedIcon::LegacyPortalSpiderDen,
        EmbeddedIcon::LegacyPortalSpriteWorld,
        EmbeddedIcon::LegacyPortalUndeadLair,
        EmbeddedIcon::LegacyPortalAbyssOfDemons,
        EmbeddedIcon::LegacyPortalCrawlingDepths,
        EmbeddedIcon::LegacyPortalWoodlandLabyrinth,
        EmbeddedIcon::LegacyPortalShatters,
    ];

    /// Embedded legacy dungeon portal for the given 0-based index, matching the
    /// sentinel ids produced by `realmhound_core::assets::get_dungeon_portal_map`
    /// (see `LEGACY_EMBED_PORTAL_BASE`). Returns `None` for out-of-range indices.
    pub fn legacy_portal(index: usize) -> Option<EmbeddedIcon> {
        Some(match index {
            0 => EmbeddedIcon::LegacyPortalPirateCave,
            1 => EmbeddedIcon::LegacyPortalSpiderDen,
            2 => EmbeddedIcon::LegacyPortalSpriteWorld,
            3 => EmbeddedIcon::LegacyPortalUndeadLair,
            4 => EmbeddedIcon::LegacyPortalAbyssOfDemons,
            5 => EmbeddedIcon::LegacyPortalCrawlingDepths,
            6 => EmbeddedIcon::LegacyPortalWoodlandLabyrinth,
            7 => EmbeddedIcon::LegacyPortalShatters,
            _ => return None,
        })
    }

    /// The raw PNG bytes embedded for this icon.
    pub fn bytes(self) -> &'static [u8] {
        match self {
            EmbeddedIcon::SeasonalChar => include_bytes!("../../assets/SeasonalChar.png"),
            EmbeddedIcon::Party => include_bytes!("../../assets/Party.png"),
            EmbeddedIcon::DungeonTypesCompleted => {
                include_bytes!("../../assets/Dungeon Types Completed.png")
            }
            EmbeddedIcon::GradeS => include_bytes!("../../assets/grade_s.png"),
            EmbeddedIcon::GradeA => include_bytes!("../../assets/grade_a.png"),
            EmbeddedIcon::GradeB => include_bytes!("../../assets/grade_b.png"),
            EmbeddedIcon::GradeC => include_bytes!("../../assets/grade_c.png"),
            EmbeddedIcon::GradeD => include_bytes!("../../assets/grade_d.png"),
            EmbeddedIcon::GradeNo => include_bytes!("../../assets/grade_no.png"),
            EmbeddedIcon::ModTypeReward => include_bytes!("../../assets/mods_reward.png"),
            EmbeddedIcon::ModTypeUnique => include_bytes!("../../assets/mods_unique.png"),
            EmbeddedIcon::ModTypeSpecial => include_bytes!("../../assets/mods_dung_unique.png"),
            EmbeddedIcon::DustGrey => include_bytes!("../../assets/grey_dust.png"),
            EmbeddedIcon::RealmeyeLogo => include_bytes!("../../assets/party/realmeye.png"),
            EmbeddedIcon::Discord => include_bytes!("../../assets/discord.png"),
            EmbeddedIcon::DiscordLogo => include_bytes!("../../assets/discord_logo.png"),
            EmbeddedIcon::NexusCrystal => include_bytes!("../../assets/NexusCrystal.png"),
            EmbeddedIcon::ForgeCommon => include_bytes!("../../assets/ForgeMaterial-Common.png"),
            EmbeddedIcon::ForgeRare => include_bytes!("../../assets/ForgeMaterial-Rare.png"),
            EmbeddedIcon::ForgeLegendary => {
                include_bytes!("../../assets/ForgeMaterial-Legendary.png")
            }
            EmbeddedIcon::ForgeMythical => {
                include_bytes!("../../assets/ForgeMaterial-Mythical.png")
            }
            EmbeddedIcon::ForgeFire => include_bytes!("../../assets/ForgeFire.png"),
            EmbeddedIcon::PartyLeader => include_bytes!("../../assets/party/party_lead.png"),
            EmbeddedIcon::PartyWatch => include_bytes!("../../assets/party/party_watch.png"),
            EmbeddedIcon::PartyBan => include_bytes!("../../assets/party/party_ban.png"),
            EmbeddedIcon::PartyMessage => include_bytes!("../../assets/party/party_message.png"),
            EmbeddedIcon::PartyPromote => include_bytes!("../../assets/party/party_promote.png"),
            EmbeddedIcon::PartyKick => include_bytes!("../../assets/party/party_kick.png"),
            EmbeddedIcon::Gift => include_bytes!("../../assets/gift_small.png"),
            EmbeddedIcon::EncounterOryx => include_bytes!("../../assets/encounter_oryx.png"),
            EmbeddedIcon::Compass => include_bytes!("../../assets/compass.png"),
            EmbeddedIcon::Bxp => include_bytes!("../../assets/bxp.png"),
            EmbeddedIcon::LegacyPortalPirateCave => {
                include_bytes!("../../assets/portals/pcave_leg.png")
            }
            EmbeddedIcon::LegacyPortalSpiderDen => {
                include_bytes!("../../assets/portals/sden_leg.png")
            }
            EmbeddedIcon::LegacyPortalSpriteWorld => {
                include_bytes!("../../assets/portals/sworld_leg.png")
            }
            EmbeddedIcon::LegacyPortalUndeadLair => {
                include_bytes!("../../assets/portals/udl_leg.png")
            }
            EmbeddedIcon::LegacyPortalAbyssOfDemons => {
                include_bytes!("../../assets/portals/abyss_leg.png")
            }
            EmbeddedIcon::LegacyPortalCrawlingDepths => {
                include_bytes!("../../assets/portals/cdepths_leg.png")
            }
            EmbeddedIcon::LegacyPortalWoodlandLabyrinth => {
                include_bytes!("../../assets/portals/wlab_leg.png")
            }
            EmbeddedIcon::LegacyPortalShatters => {
                include_bytes!("../../assets/portals/shatters_leg.png")
            }
        }
    }

    /// Stable texture-cache name for this icon.
    pub fn texture_name(self) -> &'static str {
        match self {
            EmbeddedIcon::SeasonalChar => "seasonal_char_icon",
            EmbeddedIcon::Party => "party_icon",
            EmbeddedIcon::DungeonTypesCompleted => "dungeon_types_icon",
            EmbeddedIcon::GradeS => "grade_s_icon",
            EmbeddedIcon::GradeA => "grade_a_icon",
            EmbeddedIcon::GradeB => "grade_b_icon",
            EmbeddedIcon::GradeC => "grade_c_icon",
            EmbeddedIcon::GradeD => "grade_d_icon",
            EmbeddedIcon::GradeNo => "grade_no_icon",
            EmbeddedIcon::ModTypeReward => "mods_reward_icon",
            EmbeddedIcon::ModTypeUnique => "mods_unique_icon",
            EmbeddedIcon::ModTypeSpecial => "mods_dung_unique_icon",
            EmbeddedIcon::DustGrey => "grey_dust_icon",
            EmbeddedIcon::RealmeyeLogo => "realmeye_logo_icon",
            EmbeddedIcon::Discord => "discord_icon",
            EmbeddedIcon::DiscordLogo => "discord_logo_icon",
            EmbeddedIcon::NexusCrystal => "nexus_crystal_icon",
            EmbeddedIcon::ForgeCommon => "forge_common_icon",
            EmbeddedIcon::ForgeRare => "forge_rare_icon",
            EmbeddedIcon::ForgeLegendary => "forge_legendary_icon",
            EmbeddedIcon::ForgeMythical => "forge_mythical_icon",
            EmbeddedIcon::ForgeFire => "forge_fire_icon",
            EmbeddedIcon::PartyLeader => "party_lead_icon",
            EmbeddedIcon::PartyWatch => "party_watch_icon",
            EmbeddedIcon::PartyBan => "party_ban_icon",
            EmbeddedIcon::PartyMessage => "party_message_icon",
            EmbeddedIcon::PartyPromote => "party_promote_icon",
            EmbeddedIcon::PartyKick => "party_kick_icon",
            EmbeddedIcon::Gift => "gift_small_icon",
            EmbeddedIcon::EncounterOryx => "encounter_oryx_icon",
            EmbeddedIcon::Compass => "compass_icon",
            EmbeddedIcon::Bxp => "bxp_icon",
            EmbeddedIcon::LegacyPortalPirateCave => "legacy_portal_pcave",
            EmbeddedIcon::LegacyPortalSpiderDen => "legacy_portal_sden",
            EmbeddedIcon::LegacyPortalSpriteWorld => "legacy_portal_sworld",
            EmbeddedIcon::LegacyPortalUndeadLair => "legacy_portal_udl",
            EmbeddedIcon::LegacyPortalAbyssOfDemons => "legacy_portal_abyss",
            EmbeddedIcon::LegacyPortalCrawlingDepths => "legacy_portal_cdepths",
            EmbeddedIcon::LegacyPortalWoodlandLabyrinth => "legacy_portal_wlab",
            EmbeddedIcon::LegacyPortalShatters => "legacy_portal_shatters",
        }
    }
}

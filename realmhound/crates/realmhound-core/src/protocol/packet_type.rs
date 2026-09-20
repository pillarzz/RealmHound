//! RotMG packet type definitions.
//!
//! This enum maps packet IDs to their names and directions.

/// Direction of packet flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Server to client
    Incoming,
    /// Client to server
    Outgoing,
}

/// RotMG packet types.
///
/// Each variant corresponds to a specific packet in the RotMG protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PacketType {
    // Incoming packets (Server → Client)
    Failure = 0,
    DeletePet = 4,
    QuestFetchResponse = 6,
    Ping = 8,
    NewTick = 10,
    ShowEffect = 11,
    ServerPlayerShoot = 12,
    TradeAccepted = 14,
    Goto = 18,
    NameResult = 21,
    BuyResult = 22,
    HatchPet = 23,
    GuildResult = 26,
    TradeChanged = 28,
    TradeDone = 34,
    EnemyShoot = 35,
    PlaySound = 38,
    VerifyEmail = 39,
    NewAbility = 41,
    Update = 42,
    Text = 44,
    Reconnect = 45,
    Death = 46,
    AllyShoot = 49,
    KeyInfoResponse = 63,
    Aoe = 64,
    GlobalNotification = 66,
    Notification = 67,
    ClientStat = 69,
    Damage = 75,
    ActivePetUpdate = 76,
    InvitedToGuild = 77,
    PetYardUpdate = 78,
    PasswordPrompt = 79,
    QuestObjectId = 82,
    Pic = 83,
    RealmHeroLeftMsg = 84,
    TradeStart = 86,
    EvolvePet = 87,
    TradeRequested = 88,
    MapInfo = 92,
    LoginRewardMsg = 93,
    InvResult = 95,
    QuestRedeemResponse = 96,
    AccountList = 99,
    CreateSuccess = 101,
    File = 106,
    ReskinUnlock = 107,
    NewCharacterInformation = 108,
    UnlockInformation = 109,
    QueueInformation = 112,
    ExaltationBonusChanged = 114,
    VaultUpdate = 117,
    ForgeResult = 119,
    ForgeUnlockedBlueprints = 120,
    Stats = 139,
    Unknown147 = 147,
    DamageBoost = 148,
    ClaimBpMilestoneResult = 150,
    AcceleratorActivated = 153,
    Unknown164 = 164,
    Unknown165 = 165,
    DamageWithEffect = 166,
    RealmScoreUpdate = 169,
    ClaimRewardsInfoPrompt = 170,
    ChestRewardResult = 172,
    Unknown181 = 181,
    CrucibleResponse = 183,
    Unknown190 = 190,
    PartyAction = 207,
    IncomingPartyInvite = 208,
    IncomingPartyMemberInfo = 210,
    PartyMemberAdded = 212,
    PartyListMessage = 214,
    PartyRequestResponse = 217,
    ForReconnect = 218,
    LoadingScreen = 222,

    // Outgoing packets (Client → Server)
    Teleport = 1,
    ClaimLoginRewardMsg = 3,
    RequestTrade = 5,
    JoinGuild = 7,
    PlayerText = 9,
    UseItem = 13,
    GuildRemove = 15,
    PetUpgradeRequest = 16,
    InvDrop = 19,
    OtherHit = 20,
    ActivePetUpdateRequest = 24,
    EnemyHit = 25,
    EditAccountList = 27,
    PlayerShoot = 30,
    Pong = 31,
    PetChangeSkinMsg = 33,
    AcceptTrade = 36,
    ChangeGuildRank = 37,
    SquareHit = 40,
    UsePortal = 47,
    QuestRoomMsg = 48,
    Reskin = 51,
    ResetDailyQuests = 52,
    PetChangeFormMsg = 53,
    InvSwap = 55,
    ChangeTrade = 56,
    Create = 57,
    QuestRedeem = 58,
    CreateGuild = 59,
    SetCondition = 60,
    Load = 61,
    Move = 62,
    GotoAck = 65,
    Hello = 74,
    UpdateAck = 81,
    Buy = 85,
    AoeAck = 89,
    PlayerHit = 90,
    CancelTrade = 91,
    KeyInfoRequest = 94,
    ChooseName = 97,
    QuestFetchAsk = 98,
    CheckCredits = 102,
    GroundDamage = 103,
    GuildInvite = 104,
    Escape = 105,
    QueueCancel = 113,
    RedeemExaltationReward = 115,
    ForgeRequest = 118,
    ShootAck = 121,
    ChangeAllyShoot = 122,
    GetPlayersListMessage = 123,
    ModeratorActionMessage = 124,
    CreepMoveMessage = 126,
    CustomMapDelete = 129,
    CustomMapList = 131,
    CreepHit = 133,
    PlayerCallout = 134,
    BuyRefinement = 136,
    Dash = 137,
    DashAck = 138,
    BuyCustomisationSocket = 140,
    FavourPet = 145,
    SkinRecycle = 146,
    ClaimBattlePass = 149,
    BoostBpMilestone = 151,
    ConvertSeasonalCharacter = 154,
    Retitle = 155,
    SetGraveStone = 156,
    SetAbility = 157,
    Emote = 159,
    BuyEmote = 160,
    SetTrackedSeason = 162,
    ClaimMission = 163,
    SetDiscoverable = 167,
    ClaimChestReward = 171,
    UnlockEnchantmentSlot = 173,
    UnlockEnchantment = 175,
    ApplyEnchantment = 177,
    ActivateCrucible = 180,
    CrucibleRequest = 182,
    UpgradeEnchanter = 185,
    UpgradeEnchantment = 187,
    RerollAllEnchantments = 189,
    ResetEnchantmentRerollCount = 191,
    CreatePartyMessage = 200,
    PartyActionResult = 204,
    PartyInviteResponse = 209,
    PartyJoinRequest = 215,

    // Special/internal
    //
    // Vestigial internal sentinel — NOT a real wire packet. `from_id` never
    // produces this (raw byte 255 falls through to `Unknown`), and nothing
    // constructs it. IP/region detection is handled at the TCP layer via the
    // packet capture source address (`servers::ip_to_server_name`), so no
    // synthetic IP packet is needed. Kept only to avoid a public API break.
    //
    // A synthetic id outside the single-byte wire-id range could never collide
    // with a genuine packet, but under `#[repr(u8)]` we cannot use such a value,
    // hence the 255 sentinel here.
    IpAddress = 255,

    // Unknown packet type
    Unknown = 254,
}

impl PacketType {
    /// Create a PacketType from a raw packet ID.
    pub fn from_id(id: u8) -> Self {
        match id {
            0 => Self::Failure,
            1 => Self::Teleport,
            3 => Self::ClaimLoginRewardMsg,
            4 => Self::DeletePet,
            5 => Self::RequestTrade,
            6 => Self::QuestFetchResponse,
            7 => Self::JoinGuild,
            8 => Self::Ping,
            9 => Self::PlayerText,
            10 => Self::NewTick,
            11 => Self::ShowEffect,
            12 => Self::ServerPlayerShoot,
            13 => Self::UseItem,
            14 => Self::TradeAccepted,
            15 => Self::GuildRemove,
            16 => Self::PetUpgradeRequest,
            18 => Self::Goto,
            19 => Self::InvDrop,
            20 => Self::OtherHit,
            21 => Self::NameResult,
            22 => Self::BuyResult,
            23 => Self::HatchPet,
            24 => Self::ActivePetUpdateRequest,
            25 => Self::EnemyHit,
            26 => Self::GuildResult,
            27 => Self::EditAccountList,
            28 => Self::TradeChanged,
            30 => Self::PlayerShoot,
            31 => Self::Pong,
            33 => Self::PetChangeSkinMsg,
            34 => Self::TradeDone,
            35 => Self::EnemyShoot,
            36 => Self::AcceptTrade,
            37 => Self::ChangeGuildRank,
            38 => Self::PlaySound,
            39 => Self::VerifyEmail,
            40 => Self::SquareHit,
            41 => Self::NewAbility,
            42 => Self::Update,
            44 => Self::Text,
            45 => Self::Reconnect,
            46 => Self::Death,
            47 => Self::UsePortal,
            48 => Self::QuestRoomMsg,
            49 => Self::AllyShoot,
            51 => Self::Reskin,
            52 => Self::ResetDailyQuests,
            53 => Self::PetChangeFormMsg,
            55 => Self::InvSwap,
            56 => Self::ChangeTrade,
            57 => Self::Create,
            58 => Self::QuestRedeem,
            59 => Self::CreateGuild,
            60 => Self::SetCondition,
            61 => Self::Load,
            62 => Self::Move,
            63 => Self::KeyInfoResponse,
            64 => Self::Aoe,
            65 => Self::GotoAck,
            66 => Self::GlobalNotification,
            67 => Self::Notification,
            69 => Self::ClientStat,
            74 => Self::Hello,
            75 => Self::Damage,
            76 => Self::ActivePetUpdate,
            77 => Self::InvitedToGuild,
            78 => Self::PetYardUpdate,
            79 => Self::PasswordPrompt,
            81 => Self::UpdateAck,
            82 => Self::QuestObjectId,
            83 => Self::Pic,
            84 => Self::RealmHeroLeftMsg,
            85 => Self::Buy,
            86 => Self::TradeStart,
            87 => Self::EvolvePet,
            88 => Self::TradeRequested,
            89 => Self::AoeAck,
            90 => Self::PlayerHit,
            91 => Self::CancelTrade,
            92 => Self::MapInfo,
            93 => Self::LoginRewardMsg,
            94 => Self::KeyInfoRequest,
            95 => Self::InvResult,
            96 => Self::QuestRedeemResponse,
            97 => Self::ChooseName,
            98 => Self::QuestFetchAsk,
            99 => Self::AccountList,
            101 => Self::CreateSuccess,
            102 => Self::CheckCredits,
            103 => Self::GroundDamage,
            104 => Self::GuildInvite,
            105 => Self::Escape,
            106 => Self::File,
            107 => Self::ReskinUnlock,
            108 => Self::NewCharacterInformation,
            109 => Self::UnlockInformation,
            112 => Self::QueueInformation,
            113 => Self::QueueCancel,
            114 => Self::ExaltationBonusChanged,
            115 => Self::RedeemExaltationReward,
            117 => Self::VaultUpdate,
            118 => Self::ForgeRequest,
            119 => Self::ForgeResult,
            120 => Self::ForgeUnlockedBlueprints,
            121 => Self::ShootAck,
            122 => Self::ChangeAllyShoot,
            123 => Self::GetPlayersListMessage,
            124 => Self::ModeratorActionMessage,
            126 => Self::CreepMoveMessage,
            129 => Self::CustomMapDelete,
            131 => Self::CustomMapList,
            133 => Self::CreepHit,
            134 => Self::PlayerCallout,
            136 => Self::BuyRefinement,
            137 => Self::Dash,
            138 => Self::DashAck,
            139 => Self::Stats,
            140 => Self::BuyCustomisationSocket,
            145 => Self::FavourPet,
            146 => Self::SkinRecycle,
            147 => Self::Unknown147,
            148 => Self::DamageBoost,
            149 => Self::ClaimBattlePass,
            150 => Self::ClaimBpMilestoneResult,
            151 => Self::BoostBpMilestone,
            153 => Self::AcceleratorActivated,
            154 => Self::ConvertSeasonalCharacter,
            155 => Self::Retitle,
            156 => Self::SetGraveStone,
            157 => Self::SetAbility,
            159 => Self::Emote,
            160 => Self::BuyEmote,
            162 => Self::SetTrackedSeason,
            163 => Self::ClaimMission,
            164 => Self::Unknown164,
            165 => Self::Unknown165,
            166 => Self::DamageWithEffect,
            167 => Self::SetDiscoverable,
            169 => Self::RealmScoreUpdate,
            170 => Self::ClaimRewardsInfoPrompt,
            171 => Self::ClaimChestReward,
            172 => Self::ChestRewardResult,
            173 => Self::UnlockEnchantmentSlot,
            175 => Self::UnlockEnchantment,
            177 => Self::ApplyEnchantment,
            180 => Self::ActivateCrucible,
            181 => Self::Unknown181,
            182 => Self::CrucibleRequest,
            183 => Self::CrucibleResponse,
            185 => Self::UpgradeEnchanter,
            187 => Self::UpgradeEnchantment,
            189 => Self::RerollAllEnchantments,
            190 => Self::Unknown190,
            191 => Self::ResetEnchantmentRerollCount,
            200 => Self::CreatePartyMessage,
            204 => Self::PartyActionResult,
            207 => Self::PartyAction,
            208 => Self::IncomingPartyInvite,
            209 => Self::PartyInviteResponse,
            210 => Self::IncomingPartyMemberInfo,
            212 => Self::PartyMemberAdded,
            214 => Self::PartyListMessage,
            215 => Self::PartyJoinRequest,
            217 => Self::PartyRequestResponse,
            218 => Self::ForReconnect,
            222 => Self::LoadingScreen,
            _ => Self::Unknown,
        }
    }

    /// Get the raw packet ID.
    pub fn id(self) -> u8 {
        match self {
            Self::Unknown => 254,
            Self::IpAddress => 255,
            other => other as u8,
        }
    }

    /// Get the human-readable name of this packet type.
    pub fn name(self) -> &'static str {
        match self {
            Self::Failure => "FAILURE",
            Self::Teleport => "TELEPORT",
            Self::ClaimLoginRewardMsg => "CLAIM_LOGIN_REWARD_MSG",
            Self::DeletePet => "DELETE_PET",
            Self::RequestTrade => "REQUESTTRADE",
            Self::QuestFetchResponse => "QUEST_FETCH_RESPONSE",
            Self::JoinGuild => "JOINGUILD",
            Self::Ping => "PING",
            Self::PlayerText => "PLAYERTEXT",
            Self::NewTick => "NEWTICK",
            Self::ShowEffect => "SHOWEFFECT",
            Self::ServerPlayerShoot => "SERVERPLAYERSHOOT",
            Self::UseItem => "USEITEM",
            Self::TradeAccepted => "TRADEACCEPTED",
            Self::GuildRemove => "GUILDREMOVE",
            Self::PetUpgradeRequest => "PETUPGRADEREQUEST",
            Self::Goto => "GOTO",
            Self::InvDrop => "INVDROP",
            Self::OtherHit => "OTHERHIT",
            Self::NameResult => "NAMERESULT",
            Self::BuyResult => "BUYRESULT",
            Self::HatchPet => "HATCH_PET",
            Self::ActivePetUpdateRequest => "ACTIVE_PET_UPDATE_REQUEST",
            Self::EnemyHit => "ENEMYHIT",
            Self::GuildResult => "GUILDRESULT",
            Self::EditAccountList => "EDITACCOUNTLIST",
            Self::TradeChanged => "TRADECHANGED",
            Self::PlayerShoot => "PLAYERSHOOT",
            Self::Pong => "PONG",
            Self::PetChangeSkinMsg => "PET_CHANGE_SKIN_MSG",
            Self::TradeDone => "TRADEDONE",
            Self::EnemyShoot => "ENEMYSHOOT",
            Self::AcceptTrade => "ACCEPTTRADE",
            Self::ChangeGuildRank => "CHANGEGUILDRANK",
            Self::PlaySound => "PLAYSOUND",
            Self::VerifyEmail => "VERIFY_EMAIL",
            Self::SquareHit => "SQUAREHIT",
            Self::NewAbility => "NEW_ABILITY",
            Self::Update => "UPDATE",
            Self::Text => "TEXT",
            Self::Reconnect => "RECONNECT",
            Self::Death => "DEATH",
            Self::UsePortal => "USEPORTAL",
            Self::QuestRoomMsg => "QUEST_ROOM_MSG",
            Self::AllyShoot => "ALLYSHOOT",
            Self::Reskin => "RESKIN",
            Self::ResetDailyQuests => "RESET_DAILY_QUESTS",
            Self::PetChangeFormMsg => "PET_CHANGE_FORM_MSG",
            Self::InvSwap => "INVSWAP",
            Self::ChangeTrade => "CHANGETRADE",
            Self::Create => "CREATE",
            Self::QuestRedeem => "QUEST_REDEEM",
            Self::CreateGuild => "CREATEGUILD",
            Self::SetCondition => "SETCONDITION",
            Self::Load => "LOAD",
            Self::Move => "MOVE",
            Self::KeyInfoResponse => "KEY_INFO_RESPONSE",
            Self::Aoe => "AOE",
            Self::GotoAck => "GOTOACK",
            Self::GlobalNotification => "GLOBAL_NOTIFICATION",
            Self::Notification => "NOTIFICATION",
            Self::ClientStat => "CLIENTSTAT",
            Self::Hello => "HELLO",
            Self::Damage => "DAMAGE",
            Self::ActivePetUpdate => "ACTIVEPETUPDATE",
            Self::InvitedToGuild => "INVITEDTOGUILD",
            Self::PetYardUpdate => "PETYARDUPDATE",
            Self::PasswordPrompt => "PASSWORD_PROMPT",
            Self::UpdateAck => "UPDATEACK",
            Self::QuestObjectId => "QUESTOBJID",
            Self::Pic => "PIC",
            Self::RealmHeroLeftMsg => "REALM_HERO_LEFT_MSG",
            Self::Buy => "BUY",
            Self::TradeStart => "TRADESTART",
            Self::EvolvePet => "EVOLVE_PET",
            Self::TradeRequested => "TRADEREQUESTED",
            Self::AoeAck => "AOEACK",
            Self::PlayerHit => "PLAYERHIT",
            Self::CancelTrade => "CANCELTRADE",
            Self::MapInfo => "MAPINFO",
            Self::LoginRewardMsg => "LOGIN_REWARD_MSG",
            Self::KeyInfoRequest => "KEY_INFO_REQUEST",
            Self::InvResult => "INVRESULT",
            Self::QuestRedeemResponse => "QUEST_REDEEM_RESPONSE",
            Self::ChooseName => "CHOOSENAME",
            Self::QuestFetchAsk => "QUEST_FETCH_ASK",
            Self::AccountList => "ACCOUNTLIST",
            Self::CreateSuccess => "CREATE_SUCCESS",
            Self::CheckCredits => "CHECKCREDITS",
            Self::GroundDamage => "GROUNDDAMAGE",
            Self::GuildInvite => "GUILDINVITE",
            Self::Escape => "ESCAPE",
            Self::File => "FILE",
            Self::ReskinUnlock => "RESKIN_UNLOCK",
            Self::NewCharacterInformation => "NEW_CHARACTER_INFORMATION",
            Self::UnlockInformation => "UNLOCK_INFORMATION",
            Self::QueueInformation => "QUEUE_INFORMATION",
            Self::QueueCancel => "QUEUE_CANCEL",
            Self::ExaltationBonusChanged => "EXALTATION_BONUS_CHANGED",
            Self::RedeemExaltationReward => "REDEEM_EXALTATION_REWARD",
            Self::VaultUpdate => "VAULT_UPDATE",
            Self::ForgeRequest => "FORGE_REQUEST",
            Self::ForgeResult => "FORGE_RESULT",
            Self::ForgeUnlockedBlueprints => "FORGE_UNLOCKED_BLUEPRINTS",
            Self::ShootAck => "SHOOT_ACK",
            Self::ChangeAllyShoot => "CHANGE_ALLYSHOOT",
            Self::GetPlayersListMessage => "GET_PLAYERS_LIST_MESSAGE",
            Self::ModeratorActionMessage => "MODERATOR_ACTION_MESSAGE",
            Self::CreepMoveMessage => "CREEP_MOVE_MESSAGE",
            Self::CustomMapDelete => "CUSTOM_MAP_DELETE",
            Self::CustomMapList => "CUSTOM_MAP_LIST",
            Self::CreepHit => "CREEP_HIT",
            Self::PlayerCallout => "PLAYER_CALLOUT",
            Self::BuyRefinement => "BUY_REFINEMENT",
            Self::Dash => "DASH",
            Self::DashAck => "DASH_ACK",
            Self::Stats => "STATS",
            Self::BuyCustomisationSocket => "BUY_CUSTOMISATION_SOCKET",
            Self::FavourPet => "FAVOUR_PET",
            Self::SkinRecycle => "SKIN_RECYCLE",
            Self::Unknown147 => "UNKNOWN147",
            Self::DamageBoost => "DAMAGE_BOOST",
            Self::ClaimBattlePass => "CLAIM_BATTLE_PASS",
            Self::ClaimBpMilestoneResult => "CLAIM_BP_MILESTONE_RESULT",
            Self::AcceleratorActivated => "ACCELERATOR_ACTIVATED",
            Self::BoostBpMilestone => "BOOST_BP_MILESTONE",
            Self::ConvertSeasonalCharacter => "CONVERT_SEASONAL_CHARACTER",
            Self::Retitle => "RETITLE",
            Self::SetGraveStone => "SET_GRAVE_STONE",
            Self::SetAbility => "SET_ABILITY",
            Self::Emote => "EMOTE",
            Self::BuyEmote => "BUY_EMOTE",
            Self::SetTrackedSeason => "SET_TRACKED_SEASON",
            Self::ClaimMission => "CLAIM_MISSION",
            Self::Unknown164 => "UNKNOWN164",
            Self::Unknown165 => "UNKNOWN165",
            Self::DamageWithEffect => "DAMAGE_WITH_EFFECT",
            Self::SetDiscoverable => "SET_DISCOVERABLE",
            Self::RealmScoreUpdate => "REALM_SCORE_UPDATE",
            Self::ClaimRewardsInfoPrompt => "CLAIM_REWARDS_INFO_PROMPT",
            Self::ClaimChestReward => "CLAIM_CHEST_REWARD",
            Self::ChestRewardResult => "CHEST_REWARD_RESULT",
            Self::UnlockEnchantmentSlot => "UNLOCK_ENCHANTMENT_SLOT",
            Self::UnlockEnchantment => "UNLOCK_ENCHANTMENT",
            Self::ApplyEnchantment => "APPLY_ENCHANTMENT",
            Self::ActivateCrucible => "ACTIVATE_CRUCIBLE",
            Self::Unknown181 => "UNKNOWN181",
            Self::CrucibleRequest => "CRUCIBLE_REQUEST",
            Self::CrucibleResponse => "CRUCIBLE_RESPONSE",
            Self::UpgradeEnchanter => "UPGRADE_ENCHANTER",
            Self::UpgradeEnchantment => "UPGRADE_ENCHANTMENT",
            Self::RerollAllEnchantments => "REROLL_ALL_ENCHANTMENTS",
            Self::Unknown190 => "UNKNOWN190",
            Self::ResetEnchantmentRerollCount => "RESET_ENCHANTMENT_REROLL_COUNT",
            Self::CreatePartyMessage => "CREATE_PARTY_MESSAGE",
            Self::PartyActionResult => "PARTY_ACTION_RESULT",
            Self::PartyInviteResponse => "PARTY_INVITE_RESPONSE",
            Self::IncomingPartyInvite => "INCOMING_PARTY_INVITE",
            Self::PartyAction => "PARTY_ACTION",
            Self::IncomingPartyMemberInfo => "INCOMING_PARTY_MEMBER_INFO",
            Self::PartyMemberAdded => "PARTY_MEMBER_ADDED",
            Self::PartyListMessage => "PARTY_LIST_MESSAGE",
            Self::PartyJoinRequest => "PARTY_JOIN_REQUEST",
            Self::PartyRequestResponse => "PARTY_REQUEST_RESPONSE",
            Self::ForReconnect => "FOR_RECONNECT",
            Self::LoadingScreen => "LOADING_SCREEN",
            Self::IpAddress => "IP_ADDRESS",
            Self::Unknown => "UNKNOWN",
        }
    }

    /// Get the expected direction of this packet type.
    pub fn direction(self) -> Direction {
        match self {
            // Incoming packets
            Self::Failure
            | Self::DeletePet
            | Self::QuestFetchResponse
            | Self::Ping
            | Self::NewTick
            | Self::ShowEffect
            | Self::ServerPlayerShoot
            | Self::TradeAccepted
            | Self::Goto
            | Self::NameResult
            | Self::BuyResult
            | Self::HatchPet
            | Self::GuildResult
            | Self::TradeChanged
            | Self::TradeDone
            | Self::EnemyShoot
            | Self::PlaySound
            | Self::VerifyEmail
            | Self::NewAbility
            | Self::Update
            | Self::Text
            | Self::Reconnect
            | Self::Death
            | Self::AllyShoot
            | Self::KeyInfoResponse
            | Self::Aoe
            | Self::GlobalNotification
            | Self::Notification
            | Self::ClientStat
            | Self::Damage
            | Self::ActivePetUpdate
            | Self::InvitedToGuild
            | Self::PetYardUpdate
            | Self::PasswordPrompt
            | Self::QuestObjectId
            | Self::Pic
            | Self::RealmHeroLeftMsg
            | Self::TradeStart
            | Self::EvolvePet
            | Self::TradeRequested
            | Self::MapInfo
            | Self::LoginRewardMsg
            | Self::InvResult
            | Self::QuestRedeemResponse
            | Self::AccountList
            | Self::CreateSuccess
            | Self::File
            | Self::ReskinUnlock
            | Self::NewCharacterInformation
            | Self::UnlockInformation
            | Self::QueueInformation
            | Self::ExaltationBonusChanged
            | Self::VaultUpdate
            | Self::ForgeResult
            | Self::ForgeUnlockedBlueprints
            | Self::Stats
            | Self::Unknown147
            | Self::DamageBoost
            | Self::ClaimBpMilestoneResult
            | Self::AcceleratorActivated
            | Self::Unknown164
            | Self::Unknown165
            | Self::DamageWithEffect
            | Self::RealmScoreUpdate
            | Self::ClaimRewardsInfoPrompt
            | Self::ChestRewardResult
            | Self::Unknown181
            | Self::CrucibleResponse
            | Self::Unknown190
            | Self::PartyAction
            | Self::IncomingPartyInvite
            | Self::IncomingPartyMemberInfo
            | Self::PartyMemberAdded
            | Self::PartyListMessage
            | Self::PartyRequestResponse
            | Self::ForReconnect
            | Self::LoadingScreen
            | Self::IpAddress => Direction::Incoming,

            // Everything else is outgoing
            _ => Direction::Outgoing,
        }
    }

    /// Check if this is an incoming packet (server → client).
    pub fn is_incoming(self) -> bool {
        matches!(self.direction(), Direction::Incoming)
    }

    /// Check if this is an outgoing packet (client → server).
    pub fn is_outgoing(self) -> bool {
        matches!(self.direction(), Direction::Outgoing)
    }
}

impl std::fmt::Display for PacketType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

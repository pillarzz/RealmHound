//! Packet trait definitions.

use super::super::PacketReader;
use std::fmt::Debug;
use std::io;

/// Trait for all typed RotMG packets.
///
/// Implement this trait for each packet type to enable
/// deserialization from raw bytes.
pub trait RotmgPacket: Debug + Clone + Send + Sync {
    /// Deserialize the packet from a buffer reader.
    ///
    /// The reader should be positioned at the start of the payload
    /// (after the 5-byte header has been consumed).
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self>
    where
        Self: Sized;

    /// Get a human-readable description of the packet contents.
    fn description(&self) -> String;
}

/// Enum of all parsed packet types.
///
/// This allows storing different packet types in a unified way
/// while still providing type-safe access to packet data.
#[derive(Debug, Clone)]
pub enum ParsedPacket {
    /// Chat message packet (ID 44)
    Text(super::TextPacket),
    /// Hello/connection packet (ID 74, outgoing)
    Hello(super::HelloPacket),
    /// Death packet (ID 46, incoming) - player character died
    Death(super::DeathPacket),
    /// Create packet (ID 57, outgoing) - creating a new character
    Create(super::CreatePacket),
    /// Map info packet (ID 92, incoming)
    MapInfo(super::MapInfoPacket),
    /// Create success packet (ID 101, incoming)
    CreateSuccess(super::CreateSuccessPacket),
    /// New character info packet (ID 108, incoming) - contains character XML data
    NewCharacterInfo(super::NewCharacterInfoPacket),
    /// Vault content packet (ID 117, incoming) - contains vault, materials, gifts, potions, spoils
    VaultContent(super::VaultContentPacket),
    /// Update packet (ID 42, incoming) - map updates, object stats including SEASONAL flag
    Update(super::UpdatePacket),
    /// New tick packet (ID 10, incoming) - periodic stat updates for all visible objects
    NewTick(super::NewTickPacket),
    /// Enemy hit packet (ID 25, outgoing) - player hit an enemy
    EnemyHit(super::EnemyHitPacket),
    /// Damage packet (ID 75, incoming) - damage dealt to entity
    Damage(super::DamagePacket),
    /// Quest fetch response packet (ID 6, incoming) - daily quests from Tinkerer
    QuestFetchResponse(super::QuestFetchResponsePacket),
    /// Incoming party member info packet (ID 210, incoming) - full party list
    IncomingPartyMemberInfo(super::IncomingPartyMemberInfoPacket),
    /// Party member added packet (ID 212, incoming) - new player joined party
    PartyMemberAdded(super::PartyMemberAddedPacket),
    /// Quest object ID packet (ID 82, incoming) - quest targets (realm heroes/events)
    QuestObjectId(super::QuestObjectIdPacket),
    /// Realm heroes left packet (ID 84, incoming) - count of heroes remaining
    RealmHeroesLeft(super::RealmHeroesLeftPacket),
    /// Realm score update packet (ID 169, incoming) - current realm completion score
    RealmScoreUpdate(super::RealmScoreUpdatePacket),
    /// Inventory swap packet (ID 55, outgoing) - player moving items between slots
    InvSwap(super::InvSwapPacket),
    /// Exaltation bonus changed packet (ID 114, incoming) - exaltation progress updated
    ExaltationUpdate(super::ExaltationUpdatePacket),
    /// Reconnect packet (ID 45, incoming) - server redirect with target server name
    Reconnect(super::ReconnectPacket),
    /// Failure packet (ID 0, incoming) - server error / disconnect reason
    Failure(super::FailurePacket),
    /// Ping packet (ID 8, incoming) - server heartbeat nonce
    Ping(super::PingPacket),
    /// Password prompt packet (ID 79, incoming) - prompt to enter password
    PasswordPrompt(super::PasswordPromptPacket),
    /// Queue information packet (ID 112, incoming) - login queue position
    QueueInfo(super::QueueInfoPacket),
    /// For-reconnect packet (ID 218, incoming) - opaque reconnect token
    ForReconnect(super::ForReconnectPacket),
    /// Loading screen packet (ID 222, incoming) - toggle loading screen
    LoadingScreen(super::LoadingScreenPacket),
    /// Party action result packet (ID 204, outgoing) - party action report
    PartyActionResult(super::PartyActionResultPacket),
    /// Incoming party invite packet (ID 208, incoming) - invitation to a party
    IncomingPartyInvite(super::IncomingPartyInvitePacket),
    /// Party list message packet (ID 214, incoming) - party browser listing
    PartyListMessage(super::PartyListMessagePacket),
    /// Party join request packet (ID 215, outgoing) - request to join a party
    PartyJoinRequest(super::PartyJoinRequestPacket),
    /// Party request response packet (ID 217, incoming) - response to a join request
    PartyRequestResponse(super::PartyRequestResponsePacket),
    /// Quest redeem response packet (ID 96, incoming) - quest redemption result
    QuestRedeemResponse(super::QuestRedeemResponsePacket),
    /// Claim rewards info prompt packet (ID 170, incoming) - chest reward preview
    ClaimRewardsInfoPrompt(super::ClaimRewardsInfoPromptPacket),
    /// Claim chest reward packet (ID 171, outgoing) - chest reward claim
    ClaimChestReward(super::ClaimChestRewardPacket),
    /// Chest reward result packet (ID 172, incoming) - final granted chest contents
    ChestRewardResult(super::ChestRewardResultPacket),
    /// Global notification packet (ID 66, incoming) - server-wide broadcast message
    GlobalNotification(super::GlobalNotificationPacket),
    /// Notification packet (ID 67, incoming) - in-world notification (varies by effect)
    Notification(super::NotificationPacket),
    /// Client stat packet (ID 69, incoming) - a single named account/character stat
    ClientStat(super::ClientStatPacket),
    /// Stats packet (ID 139, incoming) - character base stat state
    Stats(super::StatsPacket),
    /// Enemy shoot packet (ID 35, incoming) - enemy fired a projectile
    EnemyShoot(super::EnemyShootPacket),
    /// Server player shoot packet (ID 12, incoming) - another player fired a projectile
    ServerPlayerShoot(super::ServerPlayerShootPacket),
    /// Ally shoot packet (ID 49, incoming) - another player fired a projectile
    AllyShoot(super::AllyShootPacket),
    /// Aoe packet (ID 64, incoming) - area-of-effect grenade landed
    Aoe(super::AoePacket),
    /// Show effect packet (ID 11, incoming) - visual effect (e.g. AoE telegraph)
    ShowEffect(super::ShowEffectPacket),
    /// Goto packet (ID 18, incoming) - an entity moved to a new position
    Goto(super::GotoPacket),
    /// Play sound packet (ID 38, incoming) - client should play a sound
    PlaySound(super::PlaySoundPacket),
    /// Damage-with-effect packet (ID 166, incoming)
    DamageWithEffect(super::DamageWithEffectPacket),
    /// Damage boost packet (ID 148, incoming) - opaque 8-byte payload
    DamageBoost(super::DamageBoostPacket),
    /// Trade start packet (ID 86, incoming) - active trade initiated
    TradeStart(super::TradeStartPacket),
    /// Trade requested packet (ID 88, incoming) - another player requested a trade
    TradeRequested(super::TradeRequestedPacket),
    /// Trade accepted packet (ID 14, incoming) - both offers selected/accepted
    TradeAccepted(super::TradeAcceptedPacket),
    /// Trade changed packet (ID 28, incoming) - partner's offer selection changed
    TradeChanged(super::TradeChangedPacket),
    /// Trade done packet (ID 34, incoming) - trade completed or cancelled
    TradeDone(super::TradeDonePacket),
    /// Delete pet packet (ID 4, incoming) - a pet was deleted
    DeletePet(super::DeletePetPacket),
    /// Hatch pet packet (ID 23, incoming) - a new pet was hatched
    HatchPet(super::HatchPetPacket),
    /// Active pet update packet (ID 76, incoming) - active pet instance changed
    ActivePetUpdate(super::ActivePetUpdatePacket),
    /// Pet yard update packet (ID 78, incoming) - pet yard upgraded to a new tier
    PetYardUpdate(super::PetYardUpdatePacket),
    /// Evolve pet packet (ID 87, incoming) - a pet evolved to a new skin
    EvolvePet(super::EvolvePetPacket),
    /// Guild result packet (ID 26, incoming) - result of a guild operation
    GuildResult(super::GuildResultPacket),
    /// Invited to guild packet (ID 77, incoming) - player invited to a guild
    InvitedToGuild(super::InvitedToGuildPacket),
    /// Name result packet (ID 21, incoming) - result of a name change
    NameResult(super::NameResultPacket),
    /// Buy result packet (ID 22, incoming) - result of a shop purchase
    BuyResult(super::BuyResultPacket),
    /// Inventory result packet (ID 95, incoming) - server confirmation of an inv move
    InvResult(super::InvResultPacket),
    /// Reskin unlock packet (ID 107, incoming) - a new skin/object was unlocked
    ReskinUnlock(super::ReskinUnlockPacket),
    /// Forge result packet (ID 119, incoming) - result of an item forge action
    ForgeResult(super::ForgeResultPacket),
    /// Forge unlocked blueprints packet (ID 120, incoming) - unlocked blueprint ids
    ForgeUnlockedBlueprints(super::ForgeUnlockedBlueprintsPacket),
    /// Crucible response packet (ID 183, incoming) - crucible ids + JSON payloads
    CrucibleResponse(super::CrucibleResponsePacket),
    /// Claim BP milestone result packet (ID 150, incoming) - battle pass redeem result
    ClaimBpMilestoneResult(super::ClaimBpMilestoneResultPacket),
    AcceleratorActivated(super::AcceleratorActivatedPacket),
    /// Verify email packet (ID 39, incoming) - prompt to verify account email
    VerifyEmail(super::VerifyEmailPacket),
    /// New ability packet (ID 41, incoming) - a new ability was unlocked
    NewAbility(super::NewAbilityPacket),
    /// Pic packet (ID 83, incoming) - a bitmap image
    Pic(super::PicPacket),
    /// Login reward message packet (ID 93, incoming) - daily login reward result
    LoginRewardMsg(super::LoginRewardMsgPacket),
    /// Account list packet (ID 99, incoming) - locked/ignored account id list
    AccountList(super::AccountListPacket),
    /// File packet (ID 106, incoming) - a named file payload
    File(super::FilePacket),
    /// Unlock information packet (ID 109, incoming) - unlock type info
    UnlockInformation(super::UnlockInformationPacket),
    /// Key info response packet (ID 63, incoming) - dungeon key metadata
    KeyInfoResponse(super::KeyInfoResponsePacket),
    /// Teleport packet (ID 1, outgoing) - teleport to a player
    Teleport(super::TeleportPacket),
    /// Move packet (ID 62, outgoing) - movement/tick acknowledgement
    Move(super::MovePacket),
    /// PlayerShoot packet (ID 30, outgoing) - player fired a projectile
    PlayerShoot(super::PlayerShootPacket),
    /// GotoAck packet (ID 65, outgoing) - acknowledges a GotoPacket
    GotoAck(super::GotoAckPacket),
    /// UpdateAck packet (ID 81, outgoing) - acknowledges an UpdatePacket
    UpdateAck(super::UpdateAckPacket),
    /// GroundDamage packet (ID 103, outgoing) - damage from a ground source
    GroundDamage(super::GroundDamagePacket),
    /// OtherHit packet (ID 20, outgoing) - another object was hit
    OtherHit(super::OtherHitPacket),
    /// SquareHit packet (ID 40, outgoing) - hostile fire hit
    SquareHit(super::SquareHitPacket),
    /// PlayerHit packet (ID 90, outgoing) - the player was hit
    PlayerHit(super::PlayerHitPacket),
    /// AoeAck packet (ID 89, outgoing) - acknowledges an AoePacket
    AoeAck(super::AoeAckPacket),
    /// ShootAck packet (ID 121, outgoing) - shot acknowledgement counter
    ShootAck(super::ShootAckPacket),
    /// ChangeAllyShoot packet (ID 122, outgoing) - toggle ally projectiles
    ChangeAllyShoot(super::ChangeAllyShootPacket),
    /// SetCondition packet (ID 60, outgoing) - inflict a condition effect
    SetCondition(super::SetConditionPacket),
    /// UsePortal packet (ID 47, outgoing) - enter a portal
    UsePortal(super::UsePortalPacket),
    /// CreepMove packet (ID 126, outgoing) - move a Summoner creep
    CreepMove(super::CreepMovePacket),
    /// CreepHit packet (ID 133, outgoing) - a Summoner creep hit a target
    CreepHit(super::CreepHitPacket),
    /// Dash packet (ID 137, outgoing) - Kensei dash
    Dash(super::DashPacket),
    /// DashAck packet (ID 138, outgoing) - acknowledges a DashPacket
    DashAck(super::DashAckPacket),
    /// CreatePartyMessage packet (ID 200, outgoing) - create a party
    CreatePartyMessage(super::CreatePartyMessagePacket),
    /// PartyAction packet (ID 207, incoming) - party action on a player
    PartyAction(super::PartyActionPacket),
    /// PartyInviteResponse packet (ID 209, outgoing) - respond to a party invite
    PartyInviteResponse(super::PartyInviteResponsePacket),
    /// CustomMapDelete packet (ID 129, outgoing) - delete a custom map
    CustomMapDelete(super::CustomMapDeletePacket),
    /// CustomMapList packet (ID 131, outgoing) - request custom map list
    CustomMapList(super::CustomMapListPacket),
    /// RequestTrade packet (ID 5, outgoing) - request/accept a trade with a player
    RequestTrade(super::RequestTradePacket),
    /// AcceptTrade packet (ID 36, outgoing) - accept the current active trade
    AcceptTrade(super::AcceptTradePacket),
    /// ChangeTrade packet (ID 56, outgoing) - change the client's trade offer
    ChangeTrade(super::ChangeTradePacket),
    /// CancelTrade packet (ID 91, outgoing) - cancel the current active trade
    CancelTrade(super::CancelTradePacket),
    /// JoinGuild packet (ID 7, outgoing) - accept a pending guild invite
    JoinGuild(super::JoinGuildPacket),
    /// GuildRemove packet (ID 15, outgoing) - remove a player from the guild
    GuildRemove(super::GuildRemovePacket),
    /// ChangeGuildRank packet (ID 37, outgoing) - change a guild member's rank
    ChangeGuildRank(super::ChangeGuildRankPacket),
    /// CreateGuild packet (ID 59, outgoing) - create a new guild
    CreateGuild(super::CreateGuildPacket),
    /// GuildInvite packet (ID 104, outgoing) - invite a player to the guild
    GuildInvite(super::GuildInvitePacket),
    /// PetUpgradeRequest packet (ID 16, outgoing) - feed/fuse pets or upgrade yard
    PetUpgradeRequest(super::PetUpgradeRequestPacket),
    /// ActivePetUpdateRequest packet (ID 24, outgoing) - update the active pet
    ActivePetUpdateRequest(super::ActivePetUpdateRequestPacket),
    /// PetChangeSkinMsg packet (ID 33, outgoing) - change a pet's skin
    PetChangeSkinMsg(super::PetChangeSkinPacket),
    /// PetChangeFormMsg packet (ID 53, outgoing) - change a pet's form
    PetChangeFormMsg(super::PetChangeFormPacket),
    /// FavourPet packet (ID 145, outgoing) - favour a pet
    FavourPet(super::FavourPetPacket),
    /// UseItem packet (ID 13, outgoing) - use an ability or consumable item
    UseItem(super::UseItemPacket),
    /// InvDrop packet (ID 19, outgoing) - drop an item from the inventory
    InvDrop(super::InvDropPacket),
    /// Reskin packet (ID 51, outgoing) - activate a character skin
    Reskin(super::ReskinPacket),
    /// Buy packet (ID 85, outgoing) - buy an item
    Buy(super::BuyPacket),
    /// BuyCustomisationSocket packet (ID 140, outgoing) - buy customisation sockets
    BuyCustomisationSocket(super::BuyCustomisationSocketPacket),
    /// SkinRecycle packet (ID 146, outgoing) - recycle a skin
    SkinRecycle(super::SkinRecyclePacket),
    /// ClaimLoginRewardMsg packet (ID 3, outgoing) - claim login calendar reward
    ClaimLoginRewardMsg(super::ClaimLoginRewardMsgPacket),
    /// QuestRoomMsg packet (ID 48, outgoing) - request quest room reconnect
    QuestRoomMsg(super::QuestRoomMsgPacket),
    /// ResetDailyQuests packet (ID 52, outgoing) - reset daily quests
    ResetDailyQuests(super::ResetDailyQuestsPacket),
    /// QuestRedeem packet (ID 58, outgoing) - redeem a quest
    QuestRedeem(super::QuestRedeemPacket),
    /// QuestFetchAsk packet (ID 98, outgoing) - request latest quests
    QuestFetchAsk(super::QuestFetchAskPacket),
    /// ClaimBattlePass packet (ID 149, outgoing) - claim battle pass item
    ClaimBattlePass(super::ClaimBattlePassPacket),
    /// BoostBpMilestone packet (ID 151, outgoing) - boost a battle pass milestone
    BoostBpMilestone(super::BoostBpMilestonePacket),
    /// ConvertSeasonalCharacter packet (ID 154, outgoing) - convert seasonal character
    ConvertSeasonalCharacter(super::ConvertSeasonalCharacterPacket),
    /// SetTrackedSeason packet (ID 162, outgoing) - set the tracked season
    SetTrackedSeason(super::SetTrackedSeasonPacket),
    /// ClaimMission packet (ID 163, outgoing) - claim a season mission reward
    ClaimMission(super::ClaimMissionPacket),
    /// ForgeRequest packet (ID 118, outgoing) - forge an item
    ForgeRequest(super::ForgeRequestPacket),
    /// BuyRefinement packet (ID 136, outgoing) - buy/refund a refinement
    BuyRefinement(super::BuyRefinementPacket),
    /// UnlockEnchantmentSlot packet (ID 173, outgoing) - unlock an enchantment slot
    UnlockEnchantmentSlot(super::UnlockEnchantmentSlotPacket),
    /// UnlockEnchantment packet (ID 175, outgoing) - unlock an enchantment
    UnlockEnchantment(super::UnlockEnchantmentPacket),
    /// ApplyEnchantment packet (ID 177, outgoing) - apply/remove an enchantment
    ApplyEnchantment(super::ApplyEnchantmentPacket),
    /// ActivateCrucible packet (ID 180, outgoing) - activate/deactivate a crucible
    ActivateCrucible(super::ActivateCruciblePacket),
    /// CrucibleRequest packet (ID 182, outgoing) - request crucible rolls
    CrucibleRequest(super::CrucibleRequestPacket),
    /// UpgradeEnchanter packet (ID 185, outgoing) - upgrade the enchanter
    UpgradeEnchanter(super::UpgradeEnchanterPacket),
    /// UpgradeEnchantment packet (ID 187, outgoing) - upgrade an enchantment
    UpgradeEnchantment(super::UpgradeEnchantmentPacket),
    /// RerollAllEnchantments packet (ID 189, outgoing) - reroll all enchantments
    RerollAllEnchantments(super::RerollAllEnchantmentsPacket),
    /// ResetEnchantmentRerollCount packet (ID 191, outgoing) - reset reroll count
    ResetEnchantmentRerollCount(super::ResetEnchantmentRerollCountPacket),
    /// PlayerText packet (ID 9, outgoing) - chat message sent by the client
    PlayerText(super::PlayerTextPacket),
    /// Pong packet (ID 31, outgoing) - acknowledges a Ping
    Pong(super::PongPacket),
    /// Load packet (ID 61, outgoing) - load a character into the map
    Load(super::LoadPacket),
    /// KeyInfoRequest packet (ID 94, outgoing) - request key info for an item
    KeyInfoRequest(super::KeyInfoRequestPacket),
    /// ChooseName packet (ID 97, outgoing) - change the account name
    ChooseName(super::ChooseNamePacket),
    /// CheckCredits packet (ID 102, outgoing) - empty credits request
    CheckCredits(super::CheckCreditsPacket),
    /// Escape packet (ID 105, outgoing) - return to Nexus
    Escape(super::EscapePacket),
    /// EditAccountList packet (ID 27, outgoing) - edit an account id list
    EditAccountList(super::EditAccountListPacket),
    /// QueueCancel packet (ID 113, outgoing) - cancel queue position
    QueueCancel(super::QueueCancelPacket),
    /// GetPlayersList packet (ID 123, outgoing) - players-list request
    GetPlayersList(super::GetPlayersListPacket),
    /// ModeratorActionMessage packet (ID 124, outgoing) - staff punishment action
    ModeratorActionMessage(super::ModeratorActionMessagePacket),
    /// PlayerCallout packet (ID 134, outgoing) - object callout/ping
    PlayerCallout(super::PlayerCalloutPacket),
    /// RedeemExaltationReward packet (ID 115, outgoing) - redeem exaltation reward
    RedeemExaltationReward(super::RedeemExaltationRewardPacket),
    /// Retitle packet (ID 155, outgoing) - set title prefix/suffix
    Retitle(super::RetitlePacket),
    /// SetGraveStone packet (ID 156, outgoing) - set gravestone customization
    SetGraveStone(super::SetGraveStonePacket),
    /// SetAbility packet (ID 157, outgoing) - set active ability slot
    SetAbility(super::SetAbilityPacket),
    /// Emote packet (ID 159, outgoing) - use an emote
    Emote(super::EmotePacket),
    /// BuyEmote packet (ID 160, outgoing) - purchase an emote
    BuyEmote(super::BuyEmotePacket),
    /// SetDiscoverable packet (ID 167, outgoing) - toggle discoverable status
    SetDiscoverable(super::SetDiscoverablePacket),
    /// Unknown147 packet (ID 147, incoming) - opaque
    Unknown147(super::Unknown147Packet),
    /// Unknown164 packet (ID 164, incoming) - opaque (battle pass missions)
    Unknown164(super::Unknown164Packet),
    /// Unknown165 packet (ID 165, incoming) - opaque
    Unknown165(super::Unknown165Packet),
    /// Unknown181 packet (ID 181, incoming) - opaque
    Unknown181(super::Unknown181Packet),
    /// Unknown190 packet (ID 190, incoming) - opaque
    Unknown190(super::Unknown190Packet),
}

impl ParsedPacket {
    /// Get a description of the packet.
    pub fn description(&self) -> String {
        match self {
            ParsedPacket::Text(p) => p.description(),
            ParsedPacket::Hello(p) => p.description(),
            ParsedPacket::Death(p) => p.description(),
            ParsedPacket::Create(p) => p.description(),
            ParsedPacket::MapInfo(p) => p.description(),
            ParsedPacket::CreateSuccess(p) => p.description(),
            ParsedPacket::NewCharacterInfo(p) => p.description(),
            ParsedPacket::VaultContent(p) => p.description(),
            ParsedPacket::Update(p) => p.description(),
            ParsedPacket::NewTick(p) => p.description(),
            ParsedPacket::EnemyHit(p) => p.description(),
            ParsedPacket::Damage(p) => p.description(),
            ParsedPacket::QuestFetchResponse(p) => p.description(),
            ParsedPacket::IncomingPartyMemberInfo(p) => p.description(),
            ParsedPacket::PartyMemberAdded(p) => p.description(),
            ParsedPacket::QuestObjectId(p) => p.description(),
            ParsedPacket::RealmHeroesLeft(p) => p.description(),
            ParsedPacket::RealmScoreUpdate(p) => p.description(),
            ParsedPacket::InvSwap(p) => p.description(),
            ParsedPacket::ExaltationUpdate(p) => p.description(),
            ParsedPacket::Reconnect(p) => p.description(),
            ParsedPacket::Failure(p) => p.description(),
            ParsedPacket::Ping(p) => p.description(),
            ParsedPacket::PasswordPrompt(p) => p.description(),
            ParsedPacket::QueueInfo(p) => p.description(),
            ParsedPacket::ForReconnect(p) => p.description(),
            ParsedPacket::LoadingScreen(p) => p.description(),
            ParsedPacket::PartyActionResult(p) => p.description(),
            ParsedPacket::IncomingPartyInvite(p) => p.description(),
            ParsedPacket::PartyListMessage(p) => p.description(),
            ParsedPacket::PartyJoinRequest(p) => p.description(),
            ParsedPacket::PartyRequestResponse(p) => p.description(),
            ParsedPacket::QuestRedeemResponse(p) => p.description(),
            ParsedPacket::ClaimRewardsInfoPrompt(p) => p.description(),
            ParsedPacket::ClaimChestReward(p) => p.description(),
            ParsedPacket::ChestRewardResult(p) => p.description(),
            ParsedPacket::GlobalNotification(p) => p.description(),
            ParsedPacket::Notification(p) => p.description(),
            ParsedPacket::ClientStat(p) => p.description(),
            ParsedPacket::Stats(p) => p.description(),
            ParsedPacket::EnemyShoot(p) => p.description(),
            ParsedPacket::ServerPlayerShoot(p) => p.description(),
            ParsedPacket::AllyShoot(p) => p.description(),
            ParsedPacket::Aoe(p) => p.description(),
            ParsedPacket::ShowEffect(p) => p.description(),
            ParsedPacket::Goto(p) => p.description(),
            ParsedPacket::PlaySound(p) => p.description(),
            ParsedPacket::DamageWithEffect(p) => p.description(),
            ParsedPacket::DamageBoost(p) => p.description(),
            ParsedPacket::TradeStart(p) => p.description(),
            ParsedPacket::TradeRequested(p) => p.description(),
            ParsedPacket::TradeAccepted(p) => p.description(),
            ParsedPacket::TradeChanged(p) => p.description(),
            ParsedPacket::TradeDone(p) => p.description(),
            ParsedPacket::DeletePet(p) => p.description(),
            ParsedPacket::HatchPet(p) => p.description(),
            ParsedPacket::ActivePetUpdate(p) => p.description(),
            ParsedPacket::PetYardUpdate(p) => p.description(),
            ParsedPacket::EvolvePet(p) => p.description(),
            ParsedPacket::GuildResult(p) => p.description(),
            ParsedPacket::InvitedToGuild(p) => p.description(),
            ParsedPacket::NameResult(p) => p.description(),
            ParsedPacket::BuyResult(p) => p.description(),
            ParsedPacket::InvResult(p) => p.description(),
            ParsedPacket::ReskinUnlock(p) => p.description(),
            ParsedPacket::ForgeResult(p) => p.description(),
            ParsedPacket::ForgeUnlockedBlueprints(p) => p.description(),
            ParsedPacket::CrucibleResponse(p) => p.description(),
            ParsedPacket::ClaimBpMilestoneResult(p) => p.description(),
            ParsedPacket::AcceleratorActivated(p) => p.description(),
            ParsedPacket::VerifyEmail(p) => p.description(),
            ParsedPacket::NewAbility(p) => p.description(),
            ParsedPacket::Pic(p) => p.description(),
            ParsedPacket::LoginRewardMsg(p) => p.description(),
            ParsedPacket::AccountList(p) => p.description(),
            ParsedPacket::File(p) => p.description(),
            ParsedPacket::UnlockInformation(p) => p.description(),
            ParsedPacket::KeyInfoResponse(p) => p.description(),
            ParsedPacket::Teleport(p) => p.description(),
            ParsedPacket::Move(p) => p.description(),
            ParsedPacket::PlayerShoot(p) => p.description(),
            ParsedPacket::GotoAck(p) => p.description(),
            ParsedPacket::UpdateAck(p) => p.description(),
            ParsedPacket::GroundDamage(p) => p.description(),
            ParsedPacket::OtherHit(p) => p.description(),
            ParsedPacket::SquareHit(p) => p.description(),
            ParsedPacket::PlayerHit(p) => p.description(),
            ParsedPacket::AoeAck(p) => p.description(),
            ParsedPacket::ShootAck(p) => p.description(),
            ParsedPacket::ChangeAllyShoot(p) => p.description(),
            ParsedPacket::SetCondition(p) => p.description(),
            ParsedPacket::UsePortal(p) => p.description(),
            ParsedPacket::CreepMove(p) => p.description(),
            ParsedPacket::CreepHit(p) => p.description(),
            ParsedPacket::Dash(p) => p.description(),
            ParsedPacket::DashAck(p) => p.description(),
            ParsedPacket::CreatePartyMessage(p) => p.description(),
            ParsedPacket::PartyAction(p) => p.description(),
            ParsedPacket::PartyInviteResponse(p) => p.description(),
            ParsedPacket::CustomMapDelete(p) => p.description(),
            ParsedPacket::CustomMapList(p) => p.description(),
            ParsedPacket::RequestTrade(p) => p.description(),
            ParsedPacket::AcceptTrade(p) => p.description(),
            ParsedPacket::ChangeTrade(p) => p.description(),
            ParsedPacket::CancelTrade(p) => p.description(),
            ParsedPacket::JoinGuild(p) => p.description(),
            ParsedPacket::GuildRemove(p) => p.description(),
            ParsedPacket::ChangeGuildRank(p) => p.description(),
            ParsedPacket::CreateGuild(p) => p.description(),
            ParsedPacket::GuildInvite(p) => p.description(),
            ParsedPacket::PetUpgradeRequest(p) => p.description(),
            ParsedPacket::ActivePetUpdateRequest(p) => p.description(),
            ParsedPacket::PetChangeSkinMsg(p) => p.description(),
            ParsedPacket::PetChangeFormMsg(p) => p.description(),
            ParsedPacket::FavourPet(p) => p.description(),
            ParsedPacket::UseItem(p) => p.description(),
            ParsedPacket::InvDrop(p) => p.description(),
            ParsedPacket::Reskin(p) => p.description(),
            ParsedPacket::Buy(p) => p.description(),
            ParsedPacket::BuyCustomisationSocket(p) => p.description(),
            ParsedPacket::SkinRecycle(p) => p.description(),
            ParsedPacket::ClaimLoginRewardMsg(p) => p.description(),
            ParsedPacket::QuestRoomMsg(p) => p.description(),
            ParsedPacket::ResetDailyQuests(p) => p.description(),
            ParsedPacket::QuestRedeem(p) => p.description(),
            ParsedPacket::QuestFetchAsk(p) => p.description(),
            ParsedPacket::ClaimBattlePass(p) => p.description(),
            ParsedPacket::BoostBpMilestone(p) => p.description(),
            ParsedPacket::ConvertSeasonalCharacter(p) => p.description(),
            ParsedPacket::SetTrackedSeason(p) => p.description(),
            ParsedPacket::ClaimMission(p) => p.description(),
            ParsedPacket::ForgeRequest(p) => p.description(),
            ParsedPacket::BuyRefinement(p) => p.description(),
            ParsedPacket::UnlockEnchantmentSlot(p) => p.description(),
            ParsedPacket::UnlockEnchantment(p) => p.description(),
            ParsedPacket::ApplyEnchantment(p) => p.description(),
            ParsedPacket::ActivateCrucible(p) => p.description(),
            ParsedPacket::CrucibleRequest(p) => p.description(),
            ParsedPacket::UpgradeEnchanter(p) => p.description(),
            ParsedPacket::UpgradeEnchantment(p) => p.description(),
            ParsedPacket::RerollAllEnchantments(p) => p.description(),
            ParsedPacket::ResetEnchantmentRerollCount(p) => p.description(),
            ParsedPacket::PlayerText(p) => p.description(),
            ParsedPacket::Pong(p) => p.description(),
            ParsedPacket::Load(p) => p.description(),
            ParsedPacket::KeyInfoRequest(p) => p.description(),
            ParsedPacket::ChooseName(p) => p.description(),
            ParsedPacket::CheckCredits(p) => p.description(),
            ParsedPacket::Escape(p) => p.description(),
            ParsedPacket::EditAccountList(p) => p.description(),
            ParsedPacket::QueueCancel(p) => p.description(),
            ParsedPacket::GetPlayersList(p) => p.description(),
            ParsedPacket::ModeratorActionMessage(p) => p.description(),
            ParsedPacket::PlayerCallout(p) => p.description(),
            ParsedPacket::RedeemExaltationReward(p) => p.description(),
            ParsedPacket::Retitle(p) => p.description(),
            ParsedPacket::SetGraveStone(p) => p.description(),
            ParsedPacket::SetAbility(p) => p.description(),
            ParsedPacket::Emote(p) => p.description(),
            ParsedPacket::BuyEmote(p) => p.description(),
            ParsedPacket::SetDiscoverable(p) => p.description(),
            ParsedPacket::Unknown147(p) => p.description(),
            ParsedPacket::Unknown164(p) => p.description(),
            ParsedPacket::Unknown165(p) => p.description(),
            ParsedPacket::Unknown181(p) => p.description(),
            ParsedPacket::Unknown190(p) => p.description(),
        }
    }

    /// Check if this is a text/chat packet.
    pub fn as_text(&self) -> Option<&super::TextPacket> {
        match self {
            ParsedPacket::Text(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a hello packet.
    pub fn as_hello(&self) -> Option<&super::HelloPacket> {
        match self {
            ParsedPacket::Hello(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a map info packet.
    pub fn as_map_info(&self) -> Option<&super::MapInfoPacket> {
        match self {
            ParsedPacket::MapInfo(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a create success packet.
    pub fn as_create_success(&self) -> Option<&super::CreateSuccessPacket> {
        match self {
            ParsedPacket::CreateSuccess(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a new character info packet.
    pub fn as_new_character_info(&self) -> Option<&super::NewCharacterInfoPacket> {
        match self {
            ParsedPacket::NewCharacterInfo(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a vault content packet.
    pub fn as_vault_content(&self) -> Option<&super::VaultContentPacket> {
        match self {
            ParsedPacket::VaultContent(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is an update packet.
    pub fn as_update(&self) -> Option<&super::UpdatePacket> {
        match self {
            ParsedPacket::Update(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a new tick packet.
    pub fn as_new_tick(&self) -> Option<&super::NewTickPacket> {
        match self {
            ParsedPacket::NewTick(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is an enemy hit packet.
    pub fn as_enemy_hit(&self) -> Option<&super::EnemyHitPacket> {
        match self {
            ParsedPacket::EnemyHit(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a damage packet.
    pub fn as_damage(&self) -> Option<&super::DamagePacket> {
        match self {
            ParsedPacket::Damage(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a quest fetch response packet.
    pub fn as_quest_fetch_response(&self) -> Option<&super::QuestFetchResponsePacket> {
        match self {
            ParsedPacket::QuestFetchResponse(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is an incoming party member info packet.
    pub fn as_incoming_party_member_info(&self) -> Option<&super::IncomingPartyMemberInfoPacket> {
        match self {
            ParsedPacket::IncomingPartyMemberInfo(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a party member added packet.
    pub fn as_party_member_added(&self) -> Option<&super::PartyMemberAddedPacket> {
        match self {
            ParsedPacket::PartyMemberAdded(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a quest object ID packet.
    pub fn as_quest_object_id(&self) -> Option<&super::QuestObjectIdPacket> {
        match self {
            ParsedPacket::QuestObjectId(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a realm heroes left packet.
    pub fn as_realm_heroes_left(&self) -> Option<&super::RealmHeroesLeftPacket> {
        match self {
            ParsedPacket::RealmHeroesLeft(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a reconnect packet.
    pub fn as_reconnect(&self) -> Option<&super::ReconnectPacket> {
        match self {
            ParsedPacket::Reconnect(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a failure packet.
    pub fn as_failure(&self) -> Option<&super::FailurePacket> {
        match self {
            ParsedPacket::Failure(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a ping packet.
    pub fn as_ping(&self) -> Option<&super::PingPacket> {
        match self {
            ParsedPacket::Ping(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a password prompt packet.
    pub fn as_password_prompt(&self) -> Option<&super::PasswordPromptPacket> {
        match self {
            ParsedPacket::PasswordPrompt(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a queue information packet.
    pub fn as_queue_info(&self) -> Option<&super::QueueInfoPacket> {
        match self {
            ParsedPacket::QueueInfo(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a for-reconnect packet.
    pub fn as_for_reconnect(&self) -> Option<&super::ForReconnectPacket> {
        match self {
            ParsedPacket::ForReconnect(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a loading screen packet.
    pub fn as_loading_screen(&self) -> Option<&super::LoadingScreenPacket> {
        match self {
            ParsedPacket::LoadingScreen(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a party action result packet.
    pub fn as_party_action_result(&self) -> Option<&super::PartyActionResultPacket> {
        match self {
            ParsedPacket::PartyActionResult(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is an incoming party invite packet.
    pub fn as_incoming_party_invite(&self) -> Option<&super::IncomingPartyInvitePacket> {
        match self {
            ParsedPacket::IncomingPartyInvite(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a party list message packet.
    pub fn as_party_list_message(&self) -> Option<&super::PartyListMessagePacket> {
        match self {
            ParsedPacket::PartyListMessage(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a party join request packet.
    pub fn as_party_join_request(&self) -> Option<&super::PartyJoinRequestPacket> {
        match self {
            ParsedPacket::PartyJoinRequest(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a party request response packet.
    pub fn as_party_request_response(&self) -> Option<&super::PartyRequestResponsePacket> {
        match self {
            ParsedPacket::PartyRequestResponse(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a quest redeem response packet.
    pub fn as_quest_redeem_response(&self) -> Option<&super::QuestRedeemResponsePacket> {
        match self {
            ParsedPacket::QuestRedeemResponse(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a claim rewards info prompt packet.
    pub fn as_claim_rewards_info_prompt(&self) -> Option<&super::ClaimRewardsInfoPromptPacket> {
        match self {
            ParsedPacket::ClaimRewardsInfoPrompt(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a claim chest reward packet.
    pub fn as_claim_chest_reward(&self) -> Option<&super::ClaimChestRewardPacket> {
        match self {
            ParsedPacket::ClaimChestReward(p) => Some(p),
            _ => None,
        }
    }

    /// Check if this is a chest reward result packet.
    pub fn as_chest_reward_result(&self) -> Option<&super::ChestRewardResultPacket> {
        match self {
            ParsedPacket::ChestRewardResult(p) => Some(p),
            _ => None,
        }
    }
}

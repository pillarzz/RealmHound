//! Typed packet structures for RotMG protocol.
//!
//! This module contains the `RotmgPacket` trait and concrete packet structs
//! for each packet type in the RotMG protocol.

mod accelerator_activated;
mod accept_trade;
mod account_list;
mod activate_crucible;
mod active_pet_update;
mod active_pet_update_request;
mod ally_shoot;
mod aoe;
mod aoe_ack;
mod apply_enchantment;
mod boost_bp_milestone;
mod buy;
mod buy_customisation_socket;
mod buy_emote;
mod buy_refinement;
mod buy_result;
mod cancel_trade;
mod change_ally_shoot;
mod change_guild_rank;
mod change_trade;
mod check_credits;
mod chest_reward_result;
mod choose_name;
mod claim_battle_pass;
mod claim_bp_milestone_result;
mod claim_chest_reward;
mod claim_login_reward_msg;
mod claim_mission;
mod claim_rewards_info_prompt;
mod client_stat;
mod convert_seasonal_character;
mod create;
mod create_guild;
mod create_party_message;
mod create_success;
mod creep_hit;
mod creep_move;
mod crucible_request;
mod crucible_response;
mod custom_map_delete;
mod custom_map_list;
mod damage;
mod damage_boost;
mod damage_with_effect;
mod dash;
mod dash_ack;
mod death;
mod delete_pet;
mod edit_account_list;
mod emote;
mod enemy_hit;
mod enemy_shoot;
mod escape;
mod evolve_pet;
mod exaltation_update;
mod failure;
mod favour_pet;
mod file;
mod for_reconnect;
mod forge_request;
mod forge_result;
mod forge_unlocked_blueprints;
mod get_players_list;
mod global_notification;
mod goto;
mod goto_ack;
mod ground_damage;
mod guild_invite;
mod guild_remove;
mod guild_result;
mod hatch_pet;
mod hello;
mod incoming_party_invite;
mod incoming_party_member_info;
mod inv_drop;
mod inv_result;
mod inv_swap;
mod invited_to_guild;
mod join_guild;
mod key_info_request;
mod key_info_response;
mod load;
mod loading_screen;
mod login_reward_msg;
mod map_info;
mod moderator_action_message;
mod move_packet;
mod name_result;
mod new_ability;
mod new_character_info;
mod new_tick;
mod notification;
mod other_hit;
mod party_action;
mod party_action_result;
mod party_invite_response;
mod party_join_request;
mod party_list_message;
mod party_member_added;
mod party_request_response;
mod password_prompt;
mod pet_change_form;
mod pet_change_skin;
mod pet_upgrade_request;
mod pet_yard_update;
mod pic;
mod ping;
mod play_sound;
mod player_callout;
mod player_hit;
mod player_shoot;
mod player_text;
mod pong;
mod quest_fetch_ask;
mod quest_fetch_response;
mod quest_object_id;
mod quest_redeem;
mod quest_redeem_response;
mod quest_room_msg;
mod queue_cancel;
mod queue_info;
mod realm_heroes_left;
mod realm_score_update;
mod reconnect;
mod redeem_exaltation_reward;
mod request_trade;
mod reroll_all_enchantments;
mod reset_daily_quests;
mod reset_enchantment_reroll_count;
mod reskin;
mod reskin_unlock;
mod retitle;
mod server_player_shoot;
mod set_ability;
mod set_condition;
mod set_discoverable;
mod set_grave_stone;
mod set_tracked_season;
mod shoot_ack;
mod show_effect;
mod skin_recycle;
mod square_hit;
mod stats;
mod teleport;
mod text;
mod trade_accepted;
mod trade_changed;
mod trade_done;
mod trade_requested;
mod trade_start;
mod traits;
mod unknown147;
mod unknown164;
mod unknown165;
mod unknown181;
mod unknown190;
mod unlock_enchantment;
mod unlock_enchantment_slot;
mod unlock_information;
mod update;
mod update_ack;
mod upgrade_enchanter;
mod upgrade_enchantment;
mod use_item;
mod use_portal;
mod vault_content;
mod verify_email;

pub use accelerator_activated::AcceleratorActivatedPacket;
pub use accept_trade::AcceptTradePacket;
pub use account_list::AccountListPacket;
pub use activate_crucible::ActivateCruciblePacket;
pub use active_pet_update::ActivePetUpdatePacket;
pub use active_pet_update_request::ActivePetUpdateRequestPacket;
pub use ally_shoot::AllyShootPacket;
pub use aoe::AoePacket;
pub use aoe_ack::AoeAckPacket;
pub use apply_enchantment::ApplyEnchantmentPacket;
pub use boost_bp_milestone::BoostBpMilestonePacket;
pub use buy::BuyPacket;
pub use buy_customisation_socket::{BuyCustomisationSocketPacket, ItemBuyData};
pub use buy_emote::BuyEmotePacket;
pub use buy_refinement::BuyRefinementPacket;
pub use buy_result::BuyResultPacket;
pub use cancel_trade::CancelTradePacket;
pub use change_ally_shoot::ChangeAllyShootPacket;
pub use change_guild_rank::ChangeGuildRankPacket;
pub use change_trade::ChangeTradePacket;
pub use check_credits::CheckCreditsPacket;
pub use chest_reward_result::ChestRewardResultPacket;
pub use choose_name::ChooseNamePacket;
pub use claim_battle_pass::ClaimBattlePassPacket;
pub use claim_bp_milestone_result::ClaimBpMilestoneResultPacket;
pub use claim_chest_reward::ClaimChestRewardPacket;
pub use claim_login_reward_msg::ClaimLoginRewardMsgPacket;
pub use claim_mission::ClaimMissionPacket;
pub use claim_rewards_info_prompt::ClaimRewardsInfoPromptPacket;
pub use client_stat::ClientStatPacket;
pub use convert_seasonal_character::ConvertSeasonalCharacterPacket;
pub use create::CreatePacket;
pub use create_guild::CreateGuildPacket;
pub use create_party_message::CreatePartyMessagePacket;
pub use create_success::CreateSuccessPacket;
pub use creep_hit::CreepHitPacket;
pub use creep_move::CreepMovePacket;
pub use crucible_request::CrucibleRequestPacket;
pub use crucible_response::CrucibleResponsePacket;
pub use custom_map_delete::CustomMapDeletePacket;
pub use custom_map_list::CustomMapListPacket;
pub use damage::DamagePacket;
pub use damage_boost::DamageBoostPacket;
pub use damage_with_effect::DamageWithEffectPacket;
pub use dash::DashPacket;
pub use dash_ack::DashAckPacket;
pub use death::{DeathPacket, FameBonus};
pub use delete_pet::DeletePetPacket;
pub use edit_account_list::EditAccountListPacket;
pub use emote::EmotePacket;
pub use enemy_hit::EnemyHitPacket;
pub use enemy_shoot::EnemyShootPacket;
pub use escape::EscapePacket;
pub use evolve_pet::EvolvePetPacket;
pub use exaltation_update::ExaltationUpdatePacket;
pub use failure::FailurePacket;
pub use favour_pet::FavourPetPacket;
pub use file::FilePacket;
pub use for_reconnect::ForReconnectPacket;
pub use forge_request::ForgeRequestPacket;
pub use forge_result::ForgeResultPacket;
pub use forge_unlocked_blueprints::ForgeUnlockedBlueprintsPacket;
pub use get_players_list::GetPlayersListPacket;
pub use global_notification::GlobalNotificationPacket;
pub use goto::GotoPacket;
pub use goto_ack::GotoAckPacket;
pub use ground_damage::GroundDamagePacket;
pub use guild_invite::GuildInvitePacket;
pub use guild_remove::GuildRemovePacket;
pub use guild_result::GuildResultPacket;
pub use hatch_pet::HatchPetPacket;
pub use hello::HelloPacket;
pub use incoming_party_invite::IncomingPartyInvitePacket;
pub use incoming_party_member_info::IncomingPartyMemberInfoPacket;
pub use inv_drop::InvDropPacket;
pub use inv_result::InvResultPacket;
pub use inv_swap::{InvSwapPacket, SlotObjectData};
pub use invited_to_guild::InvitedToGuildPacket;
pub use join_guild::JoinGuildPacket;
pub use key_info_request::KeyInfoRequestPacket;
pub use key_info_response::KeyInfoResponsePacket;
pub use load::LoadPacket;
pub use loading_screen::LoadingScreenPacket;
pub use login_reward_msg::LoginRewardMsgPacket;
pub use map_info::MapInfoPacket;
pub use moderator_action_message::ModeratorActionMessagePacket;
pub use move_packet::MovePacket;
pub use name_result::NameResultPacket;
pub use new_ability::NewAbilityPacket;
pub use new_character_info::NewCharacterInfoPacket;
pub use new_tick::NewTickPacket;
pub use notification::NotificationPacket;
pub use other_hit::OtherHitPacket;
pub use party_action::PartyActionPacket;
pub use party_action_result::PartyActionResultPacket;
pub use party_invite_response::PartyInviteResponsePacket;
pub use party_join_request::PartyJoinRequestPacket;
pub use party_list_message::PartyListMessagePacket;
pub use party_member_added::PartyMemberAddedPacket;
pub use party_request_response::PartyRequestResponsePacket;
pub use password_prompt::PasswordPromptPacket;
pub use pet_change_form::PetChangeFormPacket;
pub use pet_change_skin::PetChangeSkinPacket;
pub use pet_upgrade_request::PetUpgradeRequestPacket;
pub use pet_yard_update::{PetYardType, PetYardUpdatePacket};
pub use pic::PicPacket;
pub use ping::PingPacket;
pub use play_sound::PlaySoundPacket;
pub use player_callout::PlayerCalloutPacket;
pub use player_hit::PlayerHitPacket;
pub use player_shoot::PlayerShootPacket;
pub use player_text::PlayerTextPacket;
pub use pong::PongPacket;
pub use quest_fetch_ask::QuestFetchAskPacket;
pub use quest_fetch_response::QuestFetchResponsePacket;
pub use quest_object_id::QuestObjectIdPacket;
pub use quest_redeem::QuestRedeemPacket;
pub use quest_redeem_response::QuestRedeemResponsePacket;
pub use quest_room_msg::QuestRoomMsgPacket;
pub use queue_cancel::QueueCancelPacket;
pub use queue_info::QueueInfoPacket;
pub use realm_heroes_left::RealmHeroesLeftPacket;
pub use realm_score_update::RealmScoreUpdatePacket;
pub use reconnect::ReconnectPacket;
pub use redeem_exaltation_reward::RedeemExaltationRewardPacket;
pub use request_trade::RequestTradePacket;
pub use reroll_all_enchantments::RerollAllEnchantmentsPacket;
pub use reset_daily_quests::ResetDailyQuestsPacket;
pub use reset_enchantment_reroll_count::ResetEnchantmentRerollCountPacket;
pub use reskin::ReskinPacket;
pub use reskin_unlock::ReskinUnlockPacket;
pub use retitle::RetitlePacket;
pub use server_player_shoot::ServerPlayerShootPacket;
pub use set_ability::SetAbilityPacket;
pub use set_condition::SetConditionPacket;
pub use set_discoverable::SetDiscoverablePacket;
pub use set_grave_stone::SetGraveStonePacket;
pub use set_tracked_season::SetTrackedSeasonPacket;
pub use shoot_ack::ShootAckPacket;
pub use show_effect::ShowEffectPacket;
pub use skin_recycle::SkinRecyclePacket;
pub use square_hit::SquareHitPacket;
pub use stats::{StatsPacket, StatsStateData};
pub use teleport::TeleportPacket;
pub use text::TextPacket;
pub use trade_accepted::TradeAcceptedPacket;
pub use trade_changed::TradeChangedPacket;
pub use trade_done::{TradeDonePacket, TradeResult};
pub use trade_requested::TradeRequestedPacket;
pub use trade_start::{TradeItemData, TradeStartPacket};
pub use traits::{ParsedPacket, RotmgPacket};
pub use unknown147::Unknown147Packet;
pub use unknown164::Unknown164Packet;
pub use unknown165::Unknown165Packet;
pub use unknown181::Unknown181Packet;
pub use unknown190::Unknown190Packet;
pub use unlock_enchantment::UnlockEnchantmentPacket;
pub use unlock_enchantment_slot::UnlockEnchantmentSlotPacket;
pub use unlock_information::UnlockInformationPacket;
pub use update::UpdatePacket;
pub use update_ack::UpdateAckPacket;
pub use upgrade_enchanter::UpgradeEnchanterPacket;
pub use upgrade_enchantment::UpgradeEnchantmentPacket;
pub use use_item::UseItemPacket;
pub use use_portal::UsePortalPacket;
pub use vault_content::VaultContentPacket;
pub use verify_email::VerifyEmailPacket;

use super::{PacketReader, PacketType};

/// The result of parsing a packet payload.
pub struct PacketParseResult {
    /// The typed packet when parsing succeeded.
    pub packet: Option<ParsedPacket>,
    /// Number of payload bytes consumed by the parser.
    pub bytes_consumed: usize,
    /// Whether the parser consumed the entire payload.
    pub fully_parsed: bool,
}

/// Parse a packet payload into a typed packet struct.
///
/// Returns `Some(ParsedPacket)` if the packet type is known and successfully parsed,
/// `None` if the packet type is unknown or parsing failed.
pub fn parse_packet(packet_type: PacketType, payload: &[u8]) -> Option<ParsedPacket> {
    parse_packet_with_status(packet_type, payload).packet
}

/// Parse a packet payload and retain its consumption status for diagnostics.
pub fn parse_packet_with_status(packet_type: PacketType, payload: &[u8]) -> PacketParseResult {
    let mut reader = PacketReader::new(payload);

    let result = match packet_type {
        PacketType::Text => TextPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Text),
        PacketType::Hello => HelloPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Hello),
        PacketType::Death => DeathPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Death),
        PacketType::Create => CreatePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Create),
        PacketType::MapInfo => MapInfoPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::MapInfo),
        PacketType::CreateSuccess => CreateSuccessPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::CreateSuccess),
        PacketType::NewCharacterInformation => NewCharacterInfoPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::NewCharacterInfo),
        PacketType::VaultUpdate => VaultContentPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::VaultContent),
        PacketType::Update => UpdatePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Update),
        PacketType::NewTick => NewTickPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::NewTick),
        PacketType::EnemyHit => EnemyHitPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::EnemyHit),
        PacketType::Damage => DamagePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Damage),
        PacketType::QuestFetchResponse => QuestFetchResponsePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::QuestFetchResponse),
        PacketType::IncomingPartyMemberInfo => {
            IncomingPartyMemberInfoPacket::deserialize(&mut reader)
                .ok()
                .map(ParsedPacket::IncomingPartyMemberInfo)
        }
        PacketType::PartyMemberAdded => PartyMemberAddedPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::PartyMemberAdded),
        PacketType::QuestObjectId => QuestObjectIdPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::QuestObjectId),
        PacketType::RealmHeroLeftMsg => RealmHeroesLeftPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::RealmHeroesLeft),
        PacketType::RealmScoreUpdate => RealmScoreUpdatePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::RealmScoreUpdate),
        PacketType::InvSwap => InvSwapPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::InvSwap),
        PacketType::ExaltationBonusChanged => ExaltationUpdatePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::ExaltationUpdate),
        PacketType::Reconnect => ReconnectPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Reconnect),
        PacketType::Failure => FailurePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Failure),
        PacketType::Ping => PingPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Ping),
        PacketType::PasswordPrompt => PasswordPromptPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::PasswordPrompt),
        PacketType::QueueInformation => QueueInfoPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::QueueInfo),
        PacketType::ForReconnect => ForReconnectPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::ForReconnect),
        PacketType::LoadingScreen => LoadingScreenPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::LoadingScreen),
        PacketType::PartyActionResult => PartyActionResultPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::PartyActionResult),
        PacketType::IncomingPartyInvite => IncomingPartyInvitePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::IncomingPartyInvite),
        PacketType::PartyListMessage => PartyListMessagePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::PartyListMessage),
        PacketType::PartyJoinRequest => PartyJoinRequestPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::PartyJoinRequest),
        PacketType::PartyRequestResponse => PartyRequestResponsePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::PartyRequestResponse),
        PacketType::QuestRedeemResponse => QuestRedeemResponsePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::QuestRedeemResponse),
        PacketType::ClaimRewardsInfoPrompt => {
            ClaimRewardsInfoPromptPacket::deserialize(&mut reader)
                .ok()
                .map(ParsedPacket::ClaimRewardsInfoPrompt)
        }
        PacketType::ClaimChestReward => ClaimChestRewardPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::ClaimChestReward),
        PacketType::ChestRewardResult => ChestRewardResultPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::ChestRewardResult),
        PacketType::GlobalNotification => GlobalNotificationPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::GlobalNotification),
        PacketType::Notification => NotificationPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Notification),
        PacketType::ClientStat => ClientStatPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::ClientStat),
        PacketType::Stats => StatsPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Stats),
        PacketType::EnemyShoot => EnemyShootPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::EnemyShoot),
        PacketType::ServerPlayerShoot => ServerPlayerShootPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::ServerPlayerShoot),
        PacketType::AllyShoot => AllyShootPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::AllyShoot),
        PacketType::Aoe => AoePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Aoe),
        PacketType::ShowEffect => ShowEffectPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::ShowEffect),
        PacketType::Goto => GotoPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Goto),
        PacketType::PlaySound => PlaySoundPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::PlaySound),
        PacketType::DamageWithEffect => DamageWithEffectPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::DamageWithEffect),
        PacketType::DamageBoost => DamageBoostPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::DamageBoost),
        PacketType::TradeStart => TradeStartPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::TradeStart),
        PacketType::TradeRequested => TradeRequestedPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::TradeRequested),
        PacketType::TradeAccepted => TradeAcceptedPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::TradeAccepted),
        PacketType::TradeChanged => TradeChangedPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::TradeChanged),
        PacketType::TradeDone => TradeDonePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::TradeDone),
        PacketType::DeletePet => DeletePetPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::DeletePet),
        PacketType::HatchPet => HatchPetPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::HatchPet),
        PacketType::ActivePetUpdate => ActivePetUpdatePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::ActivePetUpdate),
        PacketType::PetYardUpdate => PetYardUpdatePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::PetYardUpdate),
        PacketType::EvolvePet => EvolvePetPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::EvolvePet),
        PacketType::GuildResult => GuildResultPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::GuildResult),
        PacketType::InvitedToGuild => InvitedToGuildPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::InvitedToGuild),
        PacketType::NameResult => NameResultPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::NameResult),
        PacketType::BuyResult => BuyResultPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::BuyResult),
        PacketType::InvResult => InvResultPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::InvResult),
        PacketType::ReskinUnlock => ReskinUnlockPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::ReskinUnlock),
        PacketType::ForgeResult => ForgeResultPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::ForgeResult),
        PacketType::ForgeUnlockedBlueprints => {
            ForgeUnlockedBlueprintsPacket::deserialize(&mut reader)
                .ok()
                .map(ParsedPacket::ForgeUnlockedBlueprints)
        }
        PacketType::CrucibleResponse => CrucibleResponsePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::CrucibleResponse),
        PacketType::ClaimBpMilestoneResult => {
            ClaimBpMilestoneResultPacket::deserialize(&mut reader)
                .ok()
                .map(ParsedPacket::ClaimBpMilestoneResult)
        }
        PacketType::VerifyEmail => VerifyEmailPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::VerifyEmail),
        PacketType::NewAbility => NewAbilityPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::NewAbility),
        PacketType::Pic => PicPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Pic),
        PacketType::LoginRewardMsg => LoginRewardMsgPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::LoginRewardMsg),
        PacketType::AccountList => AccountListPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::AccountList),
        PacketType::File => FilePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::File),
        PacketType::UnlockInformation => UnlockInformationPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::UnlockInformation),
        PacketType::KeyInfoResponse => KeyInfoResponsePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::KeyInfoResponse),
        PacketType::Teleport => TeleportPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Teleport),
        PacketType::Move => MovePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Move),
        PacketType::PlayerShoot => PlayerShootPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::PlayerShoot),
        PacketType::GotoAck => GotoAckPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::GotoAck),
        PacketType::UpdateAck => UpdateAckPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::UpdateAck),
        PacketType::GroundDamage => GroundDamagePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::GroundDamage),
        PacketType::OtherHit => OtherHitPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::OtherHit),
        PacketType::SquareHit => SquareHitPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::SquareHit),
        PacketType::PlayerHit => PlayerHitPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::PlayerHit),
        PacketType::AoeAck => AoeAckPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::AoeAck),
        PacketType::ShootAck => ShootAckPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::ShootAck),
        PacketType::ChangeAllyShoot => ChangeAllyShootPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::ChangeAllyShoot),
        PacketType::SetCondition => SetConditionPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::SetCondition),
        PacketType::UsePortal => UsePortalPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::UsePortal),
        PacketType::CreepMoveMessage => CreepMovePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::CreepMove),
        PacketType::CreepHit => CreepHitPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::CreepHit),
        PacketType::Dash => DashPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Dash),
        PacketType::DashAck => DashAckPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::DashAck),
        PacketType::CreatePartyMessage => CreatePartyMessagePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::CreatePartyMessage),
        PacketType::PartyAction => PartyActionPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::PartyAction),
        PacketType::PartyInviteResponse => PartyInviteResponsePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::PartyInviteResponse),
        PacketType::CustomMapDelete => CustomMapDeletePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::CustomMapDelete),
        PacketType::CustomMapList => CustomMapListPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::CustomMapList),
        PacketType::PlayerText => PlayerTextPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::PlayerText),
        PacketType::Pong => PongPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Pong),
        PacketType::Load => LoadPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Load),
        PacketType::KeyInfoRequest => KeyInfoRequestPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::KeyInfoRequest),
        PacketType::ChooseName => ChooseNamePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::ChooseName),
        PacketType::CheckCredits => CheckCreditsPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::CheckCredits),
        PacketType::Escape => EscapePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Escape),
        PacketType::EditAccountList => EditAccountListPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::EditAccountList),
        PacketType::QueueCancel => QueueCancelPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::QueueCancel),
        PacketType::GetPlayersListMessage => GetPlayersListPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::GetPlayersList),
        PacketType::ModeratorActionMessage => {
            ModeratorActionMessagePacket::deserialize(&mut reader)
                .ok()
                .map(ParsedPacket::ModeratorActionMessage)
        }
        PacketType::PlayerCallout => PlayerCalloutPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::PlayerCallout),
        PacketType::RedeemExaltationReward => {
            RedeemExaltationRewardPacket::deserialize(&mut reader)
                .ok()
                .map(ParsedPacket::RedeemExaltationReward)
        }
        PacketType::Retitle => RetitlePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Retitle),
        PacketType::SetGraveStone => SetGraveStonePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::SetGraveStone),
        PacketType::SetAbility => SetAbilityPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::SetAbility),
        PacketType::Emote => EmotePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Emote),
        PacketType::BuyEmote => BuyEmotePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::BuyEmote),
        PacketType::SetDiscoverable => SetDiscoverablePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::SetDiscoverable),
        PacketType::Unknown147 => Unknown147Packet::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Unknown147),
        PacketType::Unknown164 => Unknown164Packet::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Unknown164),
        PacketType::Unknown165 => Unknown165Packet::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Unknown165),
        PacketType::Unknown181 => Unknown181Packet::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Unknown181),
        PacketType::Unknown190 => Unknown190Packet::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Unknown190),
        PacketType::RequestTrade => RequestTradePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::RequestTrade),
        PacketType::AcceptTrade => AcceptTradePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::AcceptTrade),
        PacketType::ChangeTrade => ChangeTradePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::ChangeTrade),
        PacketType::CancelTrade => CancelTradePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::CancelTrade),
        PacketType::JoinGuild => JoinGuildPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::JoinGuild),
        PacketType::GuildRemove => GuildRemovePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::GuildRemove),
        PacketType::ChangeGuildRank => ChangeGuildRankPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::ChangeGuildRank),
        PacketType::CreateGuild => CreateGuildPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::CreateGuild),
        PacketType::GuildInvite => GuildInvitePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::GuildInvite),
        PacketType::PetUpgradeRequest => PetUpgradeRequestPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::PetUpgradeRequest),
        PacketType::ActivePetUpdateRequest => {
            ActivePetUpdateRequestPacket::deserialize(&mut reader)
                .ok()
                .map(ParsedPacket::ActivePetUpdateRequest)
        }
        PacketType::PetChangeSkinMsg => PetChangeSkinPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::PetChangeSkinMsg),
        PacketType::PetChangeFormMsg => PetChangeFormPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::PetChangeFormMsg),
        PacketType::FavourPet => FavourPetPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::FavourPet),
        PacketType::UseItem => UseItemPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::UseItem),
        PacketType::InvDrop => InvDropPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::InvDrop),
        PacketType::Reskin => ReskinPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Reskin),
        PacketType::Buy => BuyPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::Buy),
        PacketType::BuyCustomisationSocket => {
            BuyCustomisationSocketPacket::deserialize(&mut reader)
                .ok()
                .map(ParsedPacket::BuyCustomisationSocket)
        }
        PacketType::SkinRecycle => SkinRecyclePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::SkinRecycle),
        PacketType::ClaimLoginRewardMsg => ClaimLoginRewardMsgPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::ClaimLoginRewardMsg),
        PacketType::QuestRoomMsg => QuestRoomMsgPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::QuestRoomMsg),
        PacketType::ResetDailyQuests => ResetDailyQuestsPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::ResetDailyQuests),
        PacketType::QuestRedeem => QuestRedeemPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::QuestRedeem),
        PacketType::QuestFetchAsk => QuestFetchAskPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::QuestFetchAsk),
        PacketType::ClaimBattlePass => ClaimBattlePassPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::ClaimBattlePass),
        PacketType::BoostBpMilestone => BoostBpMilestonePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::BoostBpMilestone),
        PacketType::ConvertSeasonalCharacter => {
            ConvertSeasonalCharacterPacket::deserialize(&mut reader)
                .ok()
                .map(ParsedPacket::ConvertSeasonalCharacter)
        }
        PacketType::SetTrackedSeason => SetTrackedSeasonPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::SetTrackedSeason),
        PacketType::ClaimMission => ClaimMissionPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::ClaimMission),
        PacketType::ForgeRequest => ForgeRequestPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::ForgeRequest),
        PacketType::BuyRefinement => BuyRefinementPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::BuyRefinement),
        PacketType::UnlockEnchantmentSlot => UnlockEnchantmentSlotPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::UnlockEnchantmentSlot),
        PacketType::UnlockEnchantment => UnlockEnchantmentPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::UnlockEnchantment),
        PacketType::ApplyEnchantment => ApplyEnchantmentPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::ApplyEnchantment),
        PacketType::ActivateCrucible => ActivateCruciblePacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::ActivateCrucible),
        PacketType::CrucibleRequest => CrucibleRequestPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::CrucibleRequest),
        PacketType::UpgradeEnchanter => UpgradeEnchanterPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::UpgradeEnchanter),
        PacketType::UpgradeEnchantment => UpgradeEnchantmentPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::UpgradeEnchantment),
        PacketType::RerollAllEnchantments => RerollAllEnchantmentsPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::RerollAllEnchantments),
        PacketType::ResetEnchantmentRerollCount => {
            ResetEnchantmentRerollCountPacket::deserialize(&mut reader)
                .ok()
                .map(ParsedPacket::ResetEnchantmentRerollCount)
        }
        PacketType::AcceleratorActivated => AcceleratorActivatedPacket::deserialize(&mut reader)
            .ok()
            .map(ParsedPacket::AcceleratorActivated),
        // Add more packet types here as they are implemented
        _ => None,
    };

    PacketParseResult {
        packet: result,
        bytes_consumed: reader.position(),
        fully_parsed: reader.is_fully_parsed(),
    }
}

/// Check if a packet type has a parser implementation.
pub fn has_parser(packet_type: PacketType) -> bool {
    matches!(
        packet_type,
        PacketType::Text
            | PacketType::Hello
            | PacketType::Death
            | PacketType::Create
            | PacketType::MapInfo
            | PacketType::CreateSuccess
            | PacketType::NewCharacterInformation
            | PacketType::VaultUpdate
            | PacketType::Update
            | PacketType::NewTick
            | PacketType::EnemyHit
            | PacketType::Damage
            | PacketType::QuestFetchResponse
            | PacketType::IncomingPartyMemberInfo
            | PacketType::PartyMemberAdded
            | PacketType::QuestObjectId
            | PacketType::RealmHeroLeftMsg
            | PacketType::RealmScoreUpdate
            | PacketType::InvSwap
            | PacketType::ExaltationBonusChanged
            | PacketType::Reconnect
            | PacketType::Failure
            | PacketType::Ping
            | PacketType::PasswordPrompt
            | PacketType::QueueInformation
            | PacketType::ForReconnect
            | PacketType::LoadingScreen
            | PacketType::PartyActionResult
            | PacketType::IncomingPartyInvite
            | PacketType::PartyListMessage
            | PacketType::PartyJoinRequest
            | PacketType::PartyRequestResponse
            | PacketType::QuestRedeemResponse
            | PacketType::ClaimRewardsInfoPrompt
            | PacketType::ClaimChestReward
            | PacketType::ChestRewardResult
            | PacketType::GlobalNotification
            | PacketType::Notification
            | PacketType::ClientStat
            | PacketType::Stats
            | PacketType::EnemyShoot
            | PacketType::ServerPlayerShoot
            | PacketType::AllyShoot
            | PacketType::Aoe
            | PacketType::ShowEffect
            | PacketType::Goto
            | PacketType::PlaySound
            | PacketType::DamageWithEffect
            | PacketType::DamageBoost
            | PacketType::TradeStart
            | PacketType::TradeRequested
            | PacketType::TradeAccepted
            | PacketType::TradeChanged
            | PacketType::TradeDone
            | PacketType::DeletePet
            | PacketType::HatchPet
            | PacketType::ActivePetUpdate
            | PacketType::PetYardUpdate
            | PacketType::EvolvePet
            | PacketType::GuildResult
            | PacketType::InvitedToGuild
            | PacketType::NameResult
            | PacketType::BuyResult
            | PacketType::InvResult
            | PacketType::ReskinUnlock
            | PacketType::ForgeResult
            | PacketType::ForgeUnlockedBlueprints
            | PacketType::CrucibleResponse
            | PacketType::ClaimBpMilestoneResult
            | PacketType::AcceleratorActivated
            | PacketType::VerifyEmail
            | PacketType::NewAbility
            | PacketType::Pic
            | PacketType::LoginRewardMsg
            | PacketType::AccountList
            | PacketType::File
            | PacketType::UnlockInformation
            | PacketType::KeyInfoResponse
            | PacketType::Teleport
            | PacketType::Move
            | PacketType::PlayerShoot
            | PacketType::GotoAck
            | PacketType::UpdateAck
            | PacketType::GroundDamage
            | PacketType::OtherHit
            | PacketType::SquareHit
            | PacketType::PlayerHit
            | PacketType::AoeAck
            | PacketType::ShootAck
            | PacketType::ChangeAllyShoot
            | PacketType::SetCondition
            | PacketType::UsePortal
            | PacketType::CreepMoveMessage
            | PacketType::CreepHit
            | PacketType::Dash
            | PacketType::DashAck
            | PacketType::CreatePartyMessage
            | PacketType::PartyAction
            | PacketType::PartyInviteResponse
            | PacketType::CustomMapDelete
            | PacketType::CustomMapList
            | PacketType::RequestTrade
            | PacketType::AcceptTrade
            | PacketType::ChangeTrade
            | PacketType::CancelTrade
            | PacketType::JoinGuild
            | PacketType::GuildRemove
            | PacketType::ChangeGuildRank
            | PacketType::CreateGuild
            | PacketType::GuildInvite
            | PacketType::PetUpgradeRequest
            | PacketType::ActivePetUpdateRequest
            | PacketType::PetChangeSkinMsg
            | PacketType::PetChangeFormMsg
            | PacketType::FavourPet
            | PacketType::UseItem
            | PacketType::InvDrop
            | PacketType::Reskin
            | PacketType::Buy
            | PacketType::BuyCustomisationSocket
            | PacketType::SkinRecycle
            | PacketType::ClaimLoginRewardMsg
            | PacketType::QuestRoomMsg
            | PacketType::ResetDailyQuests
            | PacketType::QuestRedeem
            | PacketType::QuestFetchAsk
            | PacketType::ClaimBattlePass
            | PacketType::BoostBpMilestone
            | PacketType::ConvertSeasonalCharacter
            | PacketType::SetTrackedSeason
            | PacketType::ClaimMission
            | PacketType::ForgeRequest
            | PacketType::BuyRefinement
            | PacketType::UnlockEnchantmentSlot
            | PacketType::UnlockEnchantment
            | PacketType::ApplyEnchantment
            | PacketType::ActivateCrucible
            | PacketType::CrucibleRequest
            | PacketType::UpgradeEnchanter
            | PacketType::UpgradeEnchantment
            | PacketType::RerollAllEnchantments
            | PacketType::ResetEnchantmentRerollCount
            | PacketType::PlayerText
            | PacketType::Pong
            | PacketType::Load
            | PacketType::KeyInfoRequest
            | PacketType::ChooseName
            | PacketType::CheckCredits
            | PacketType::Escape
            | PacketType::EditAccountList
            | PacketType::QueueCancel
            | PacketType::GetPlayersListMessage
            | PacketType::ModeratorActionMessage
            | PacketType::PlayerCallout
            | PacketType::RedeemExaltationReward
            | PacketType::Retitle
            | PacketType::SetGraveStone
            | PacketType::SetAbility
            | PacketType::Emote
            | PacketType::BuyEmote
            | PacketType::SetDiscoverable
            | PacketType::Unknown147
            | PacketType::Unknown164
            | PacketType::Unknown165
            | PacketType::Unknown181
            | PacketType::Unknown190
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_packet_with_status_marks_complete_ping_as_fully_parsed() {
        let result = parse_packet_with_status(PacketType::Ping, &42_i32.to_be_bytes());

        assert!(result.packet.is_some());
        assert!(result.fully_parsed);
        assert_eq!(result.bytes_consumed, 4);
    }

    #[test]
    fn parse_packet_with_status_preserves_ping_suffix_as_unparsed() {
        let result = parse_packet_with_status(PacketType::Ping, &[0, 0, 0, 42, 0xAA, 0xBB]);

        assert!(result.packet.is_some());
        assert!(!result.fully_parsed);
        assert_eq!(result.bytes_consumed, 4);
    }

    #[test]
    fn parse_packet_with_status_decodes_19_byte_damage_with_effect() {
        let payload = [
            0x00, 0x00, 0x03, 0xaf, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff,
            0x16, 0x40, 0xe0, 0x00, 0x00,
        ];
        let result = parse_packet_with_status(PacketType::from_id(166), &payload);

        assert!(matches!(
            result.packet,
            Some(ParsedPacket::DamageWithEffect(_))
        ));
        assert!(result.fully_parsed);
        assert_eq!(result.bytes_consumed, payload.len());
    }
}

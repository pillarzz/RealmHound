//! RotMG protocol definitions.
//!
//! This module contains packet types, structures, and parsing logic
//! for the Realm of the Mad God network protocol.

pub mod data;
mod packet;
mod packet_type;
pub mod packets;
mod reader;
pub mod servers;

pub use packet::{Packet, PacketHeader};
pub use packet_type::PacketType;
pub use packets::{
    has_parser, parse_packet, parse_packet_with_status, CreatePacket, CreateSuccessPacket,
    DeathPacket, HelloPacket, IncomingPartyMemberInfoPacket, InvSwapPacket, MapInfoPacket,
    NewCharacterInfoPacket, NewTickPacket, PacketParseResult, ParsedPacket, PartyMemberAddedPacket,
    PartyRequestResponsePacket, QuestFetchResponsePacket, RotmgPacket, SlotObjectData, TextPacket,
    UpdatePacket, VaultContentPacket,
};
pub use reader::PacketReader;
pub use servers::{ip_to_server_name, short_server_name};

//! ChangeGuildRankPacket implementation.
//!
//! Sent to change the guild rank of a member in the player's guild.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ChangeGuildRankPacket (ID 37) - Outgoing
#[derive(Debug, Clone)]
pub struct ChangeGuildRankPacket {
    /// The name of the player whose rank will change.
    pub name: String,
    /// The new rank of the player.
    pub guild_rank: i32,
}

impl RotmgPacket for ChangeGuildRankPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let name = reader.read_string()?;
        let guild_rank = reader.read_i32()?;
        Ok(Self { name, guild_rank })
    }

    fn description(&self) -> String {
        format!(
            "ChangeGuildRank: name={}, rank={}",
            self.name, self.guild_rank
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&3u16.to_be_bytes());
        data.extend_from_slice(b"Eve");
        data.extend_from_slice(&20i32.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = ChangeGuildRankPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.name, "Eve");
        assert_eq!(packet.guild_rank, 20);
        assert!(reader.is_fully_parsed());
    }
}

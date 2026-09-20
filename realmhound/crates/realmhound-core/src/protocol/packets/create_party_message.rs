//! CreatePartyMessagePacket implementation.
//!
//! Sent to create a party with the given parameters.
//!

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// CreatePartyMessagePacket (ID 200) - Outgoing
#[derive(Debug, Clone)]
pub struct CreatePartyMessagePacket {
    /// The party description.
    pub description: String,
    /// The party power level.
    pub power_level: i16,
    /// The party size.
    pub party_size: i8,
    /// The party activity.
    pub activity: i8,
    /// The maxed-stats requirement.
    pub maxed_stats: i8,
    /// The server dropdown list index.
    pub server_dropdown_list: i8,
    /// The party privacy setting.
    pub privacy: i8,
}

impl RotmgPacket for CreatePartyMessagePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let description = reader.read_string()?;
        let power_level = reader.read_i16()?;
        let party_size = reader.read_byte()? as i8;
        let activity = reader.read_byte()? as i8;
        let maxed_stats = reader.read_byte()? as i8;
        let server_dropdown_list = reader.read_byte()? as i8;
        let privacy = reader.read_byte()? as i8;

        Ok(Self {
            description,
            power_level,
            party_size,
            activity,
            maxed_stats,
            server_dropdown_list,
            privacy,
        })
    }

    fn description(&self) -> String {
        format!(
            "CreatePartyMessage: desc={}, powerLevel={}, size={}",
            self.description, self.power_level, self.party_size
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
        data.extend_from_slice(b"abc");
        data.extend_from_slice(&50i16.to_be_bytes()); // powerLevel
        data.push(4); // partySize
        data.push(1); // activity
        data.push(0); // maxedStats
        data.push(2); // serverDropdownList
        data.push(1); // privacy

        let mut reader = PacketReader::new(&data);
        let packet = CreatePartyMessagePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.description, "abc");
        assert_eq!(packet.power_level, 50);
        assert_eq!(packet.party_size, 4);
        assert_eq!(packet.activity, 1);
        assert_eq!(packet.server_dropdown_list, 2);
        assert_eq!(packet.privacy, 1);
        assert!(reader.is_fully_parsed());
    }
}

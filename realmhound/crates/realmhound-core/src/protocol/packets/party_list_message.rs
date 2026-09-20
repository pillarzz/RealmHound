//! PartyListMessage packet implementation.
//!
//! Received with the list of joinable parties (party browser).

use super::traits::RotmgPacket;
use super::PacketReader;
use crate::protocol::data::PartyData;
use std::io;

/// Sanity cap on the number of parties in a single listing.
const MAX_PARTIES: i16 = 1000;

/// PartyListMessage packet (ID 214) - Incoming
///
/// Sent by the server with the list of parties shown in the party browser.
#[derive(Debug, Clone)]
pub struct PartyListMessagePacket {
    /// Leading count byte (sent separately from the array length).
    pub count: u8,
    /// The list of joinable parties.
    pub parties: Vec<PartyData>,
}

impl RotmgPacket for PartyListMessagePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let count = reader.read_byte()?;

        // The array length is a separate i16 prefix, NOT `count`.
        let parties_len = reader.read_i16()?;
        if parties_len < 0 || parties_len > MAX_PARTIES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid party count: {}", parties_len),
            ));
        }
        let parties_len = parties_len as usize;
        let mut parties = Vec::with_capacity(parties_len);
        for _ in 0..parties_len {
            parties.push(PartyData::deserialize(reader)?);
        }

        Ok(Self { count, parties })
    }

    fn description(&self) -> String {
        format!(
            "PartyListMessage: {} parties (count={})",
            self.parties.len(),
            self.count
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_party(data: &mut Vec<u8>, desc: &str, party_id: i32) {
        data.extend_from_slice(&(desc.len() as u16).to_be_bytes());
        data.extend_from_slice(desc.as_bytes());
        data.extend_from_slice(&party_id.to_be_bytes());
        data.extend_from_slice(&0i16.to_be_bytes()); // power_level
        data.push(2); // current_players
        data.push(8); // max_players
        data.push(0); // party_type
        data.push(1); // privacy
        data.push(0); // required_maxed_stats
        data.push(3); // server
    }

    #[test]
    fn test_deserialize_multiple() {
        let mut data = Vec::new();
        data.push(5); // count byte (intentionally != parties.len())
        data.extend_from_slice(&2i16.to_be_bytes()); // array length
        push_party(&mut data, "First", 100);
        push_party(&mut data, "Second", 200);

        let mut reader = PacketReader::new(&data);
        let packet = PartyListMessagePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.count, 5);
        assert_eq!(packet.parties.len(), 2);
        assert_eq!(packet.parties[0].description, "First");
        assert_eq!(packet.parties[0].party_id, 100);
        assert_eq!(packet.parties[0].current_players, 2);
        assert_eq!(packet.parties[1].description, "Second");
        assert_eq!(packet.parties[1].server, 3);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_empty_list() {
        let mut data = Vec::new();
        data.push(0);
        data.extend_from_slice(&0i16.to_be_bytes());
        let mut reader = PacketReader::new(&data);
        let packet = PartyListMessagePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.parties.len(), 0);
    }

    #[test]
    fn test_negative_length_rejected() {
        let mut data = Vec::new();
        data.push(0);
        data.extend_from_slice(&(-1i16).to_be_bytes());
        let mut reader = PacketReader::new(&data);
        assert!(PartyListMessagePacket::deserialize(&mut reader).is_err());
    }
}

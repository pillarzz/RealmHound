//! QUEST_FETCH_RESPONSE packet (ID 6) - Daily quests from Tinkerer.
//!
//! Received when entering the Daily Quest Room (Tinkerer).
//! Contains all available daily quests with their requirements and rewards.

use super::super::data::QuestData;
use super::super::PacketReader;
use super::traits::RotmgPacket;
use std::io;

/// Quest fetch response packet received from the server.
///
/// This packet is sent when the player enters the Daily Quest Room
/// and contains all available quests from the Tinkerer.
#[derive(Debug, Clone)]
pub struct QuestFetchResponsePacket {
    /// The list of available quests
    pub quests: Vec<QuestData>,
    /// The cost in gold for the next quest refresh
    pub next_refresh_price: i16,
    /// Unknown trailing int (added in recent protocol update)
    pub unknown: i32,
}

impl RotmgPacket for QuestFetchResponsePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        // Read quest count (short)
        let quest_count = reader.read_i16()?;
        if quest_count < 0 || quest_count > 1000 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid quest count: {}", quest_count),
            ));
        }
        let quest_count = quest_count as usize;

        // Read each quest
        let mut quests = Vec::with_capacity(quest_count);
        for _ in 0..quest_count {
            quests.push(QuestData::deserialize(reader)?);
        }

        // Read refresh price
        let next_refresh_price = reader.read_i16()?;

        // Unknown trailing int (added in recent protocol update)
        let unknown = if reader.remaining() >= 4 {
            reader.read_i32()?
        } else {
            0
        };

        Ok(Self {
            quests,
            next_refresh_price,
            unknown,
        })
    }

    fn description(&self) -> String {
        let displayable: Vec<_> = self.quests.iter().filter(|q| q.should_display()).collect();

        format!(
            "{} quests ({} displayable), refresh cost: {} gold",
            self.quests.len(),
            displayable.len(),
            self.next_refresh_price
        )
    }
}

impl QuestFetchResponsePacket {
    /// Get quests that should be displayed (filters out completed non-repeatable quests).
    pub fn displayable_quests(&self) -> Vec<&QuestData> {
        self.quests.iter().filter(|q| q.should_display()).collect()
    }

    /// Get quests sorted by category.
    pub fn quests_sorted_by_category(&self) -> Vec<&QuestData> {
        let mut quests: Vec<_> = self.quests.iter().collect();
        quests.sort_by_key(|q| q.category);
        quests
    }

    /// Get displayable quests sorted by category.
    pub fn displayable_quests_sorted(&self) -> Vec<&QuestData> {
        let mut quests: Vec<_> = self.quests.iter().filter(|q| q.should_display()).collect();
        quests.sort_by_key(|q| q.category);
        quests
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_string_bytes(s: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        let len = s.len() as u16;
        bytes.extend_from_slice(&len.to_be_bytes());
        bytes.extend_from_slice(s.as_bytes());
        bytes
    }

    fn make_quest_bytes(
        id: &str,
        name: &str,
        category: i32,
        completed: bool,
        repeatable: bool,
    ) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&make_string_bytes(id));
        data.extend_from_slice(&make_string_bytes(name));
        data.extend_from_slice(&make_string_bytes("Description"));
        data.extend_from_slice(&make_string_bytes(""));
        data.extend_from_slice(&category.to_be_bytes());
        data.extend_from_slice(&0i32.to_be_bytes()); // unknown_int
        data.extend_from_slice(&0i16.to_be_bytes()); // 0 requirements
        data.extend_from_slice(&0i16.to_be_bytes()); // 0 rewards
        data.push(if completed { 1 } else { 0 });
        data.push(0); // item_of_choice
        data.push(if repeatable { 1 } else { 0 });
        data
    }

    #[test]
    fn test_quest_fetch_response_empty() {
        let mut data = Vec::new();
        // 0 quests
        data.extend_from_slice(&0i16.to_be_bytes());
        // refresh price: 50 gold
        data.extend_from_slice(&50i16.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = QuestFetchResponsePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.quests.len(), 0);
        assert_eq!(packet.next_refresh_price, 50);
    }

    #[test]
    fn test_quest_fetch_response_multiple_quests() {
        let mut data = Vec::new();

        // 3 quests
        data.extend_from_slice(&3i16.to_be_bytes());

        // Quest 1: category 2, not completed
        data.extend_from_slice(&make_quest_bytes("q1", "Quest One", 2, false, false));

        // Quest 2: category 1, completed (non-repeatable - should be hidden)
        data.extend_from_slice(&make_quest_bytes("q2", "Quest Two", 1, true, false));

        // Quest 3: category 0, completed but repeatable (should show)
        data.extend_from_slice(&make_quest_bytes("q3", "Quest Three", 0, true, true));

        // refresh price: 100 gold
        data.extend_from_slice(&100i16.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = QuestFetchResponsePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.quests.len(), 3);
        assert_eq!(packet.next_refresh_price, 100);

        // Check displayable quests (should exclude q2)
        let displayable = packet.displayable_quests();
        assert_eq!(displayable.len(), 2);
        assert!(displayable.iter().any(|q| q.id == "q1"));
        assert!(displayable.iter().any(|q| q.id == "q3"));
        assert!(!displayable.iter().any(|q| q.id == "q2"));

        // Check sorted by category
        let sorted = packet.displayable_quests_sorted();
        assert_eq!(sorted.len(), 2);
        assert_eq!(sorted[0].id, "q3"); // category 0
        assert_eq!(sorted[1].id, "q1"); // category 2
    }

    #[test]
    fn test_quest_fetch_response_description() {
        let mut data = Vec::new();
        data.extend_from_slice(&2i16.to_be_bytes());
        data.extend_from_slice(&make_quest_bytes("q1", "Quest One", 0, false, false));
        data.extend_from_slice(&make_quest_bytes("q2", "Quest Two", 0, true, false));
        data.extend_from_slice(&25i16.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = QuestFetchResponsePacket::deserialize(&mut reader).unwrap();

        let desc = packet.description();
        assert!(desc.contains("2 quests"));
        assert!(desc.contains("1 displayable"));
        assert!(desc.contains("25 gold"));
    }
}

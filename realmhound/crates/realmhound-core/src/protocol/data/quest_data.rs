//! Quest data structure used in QuestFetchResponse packets.
//!
//! This represents a single daily quest from the Tinkerer.

use crate::protocol::PacketReader;
use serde::{Deserialize, Serialize};
use std::io;

/// Individual quest data from the Tinkerer.
///
/// Contains all information about a daily quest including
/// requirements (items needed) and rewards (items received).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestData {
    /// The unique identifier for this quest
    pub id: String,
    /// The display name of this quest
    pub name: String,
    /// The description of this quest
    pub description: String,
    /// The expiration time of this quest (as string timestamp)
    pub expiration: String,
    /// The category of this quest (used for sorting)
    pub category: i32,
    /// Unknown integer field
    pub unknown_int: i32,
    /// List of item IDs required to complete this quest
    pub requirements: Vec<i32>,
    /// List of item IDs awarded upon completion
    pub rewards: Vec<i32>,
    /// Whether this quest has been completed
    pub completed: bool,
    /// Whether the player must choose one reward from multiple options
    pub item_of_choice: bool,
    /// Whether this quest can be completed multiple times
    pub repeatable: bool,
}

impl QuestData {
    /// Deserialize quest data from a packet reader.
    ///
    /// The deserialization order is:
    /// 1. id (String)
    /// 2. name (String)
    /// 3. description (String)
    /// 4. expiration (String)
    /// 5. category (int)
    /// 6. unknownInt (int)
    /// 7. requirements (short count, then int[] item IDs)
    /// 8. rewards (short count, then int[] item IDs)
    /// 9. completed (boolean)
    /// 10. itemOfChoice (boolean)
    /// 11. repeatable (boolean)
    pub fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let id = reader.read_string()?;
        let name = reader.read_string()?;
        let description = reader.read_string()?;
        let expiration = reader.read_string()?;
        let category = reader.read_i32()?;
        let unknown_int = reader.read_i32()?;

        // Read requirements array
        let req_count = reader.read_i16()?.max(0) as usize;
        if req_count > 1000 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid requirements count: {}", req_count),
            ));
        }
        let mut requirements = Vec::with_capacity(req_count);
        for _ in 0..req_count {
            requirements.push(reader.read_i32()?);
        }

        // Read rewards array
        let reward_count = reader.read_i16()?.max(0) as usize;
        if reward_count > 1000 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid rewards count: {}", reward_count),
            ));
        }
        let mut rewards = Vec::with_capacity(reward_count);
        for _ in 0..reward_count {
            rewards.push(reader.read_i32()?);
        }

        let completed = reader.read_bool()?;
        let item_of_choice = reader.read_bool()?;
        let repeatable = reader.read_bool()?;

        Ok(Self {
            id,
            name,
            description,
            expiration,
            category,
            unknown_int,
            requirements,
            rewards,
            completed,
            item_of_choice,
            repeatable,
        })
    }

    /// Check if this quest should be displayed.
    ///
    /// Non-repeatable quests that are completed should be hidden.
    pub fn should_display(&self) -> bool {
        self.repeatable || !self.completed
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

    #[test]
    fn test_quest_data_deserialize() {
        let mut data = Vec::new();

        // id: "daily_quest_1"
        data.extend_from_slice(&make_string_bytes("daily_quest_1"));
        // name: "Oryx's Sanctuary Challenge"
        data.extend_from_slice(&make_string_bytes("Oryx's Sanctuary Challenge"));
        // description: "Complete dungeon marks"
        data.extend_from_slice(&make_string_bytes("Complete dungeon marks"));
        // expiration: "2026-01-23T00:00:00Z"
        data.extend_from_slice(&make_string_bytes("2026-01-23T00:00:00Z"));
        // category: 1
        data.extend_from_slice(&1i32.to_be_bytes());
        // unknown_int: 0
        data.extend_from_slice(&0i32.to_be_bytes());
        // requirements: 3 items [0x1234, 0x5678, 0x9ABC]
        data.extend_from_slice(&3i16.to_be_bytes());
        data.extend_from_slice(&0x1234i32.to_be_bytes());
        data.extend_from_slice(&0x5678i32.to_be_bytes());
        data.extend_from_slice(&0x9ABCi32.to_be_bytes());
        // rewards: 1 item [0xDEF0]
        data.extend_from_slice(&1i16.to_be_bytes());
        data.extend_from_slice(&0xDEF0i32.to_be_bytes());
        // completed: false
        data.push(0);
        // item_of_choice: false
        data.push(0);
        // repeatable: false
        data.push(0);

        let mut reader = PacketReader::new(&data);
        let quest = QuestData::deserialize(&mut reader).unwrap();

        assert_eq!(quest.id, "daily_quest_1");
        assert_eq!(quest.name, "Oryx's Sanctuary Challenge");
        assert_eq!(quest.description, "Complete dungeon marks");
        assert_eq!(quest.expiration, "2026-01-23T00:00:00Z");
        assert_eq!(quest.category, 1);
        assert_eq!(quest.unknown_int, 0);
        assert_eq!(quest.requirements, vec![0x1234, 0x5678, 0x9ABC]);
        assert_eq!(quest.rewards, vec![0xDEF0]);
        assert!(!quest.completed);
        assert!(!quest.item_of_choice);
        assert!(!quest.repeatable);
        assert!(quest.should_display());
    }

    #[test]
    fn test_quest_data_completed_non_repeatable() {
        let mut data = Vec::new();

        // Minimal quest data for testing should_display
        data.extend_from_slice(&make_string_bytes("quest_2"));
        data.extend_from_slice(&make_string_bytes("Test Quest"));
        data.extend_from_slice(&make_string_bytes("Description"));
        data.extend_from_slice(&make_string_bytes(""));
        data.extend_from_slice(&0i32.to_be_bytes()); // category
        data.extend_from_slice(&0i32.to_be_bytes()); // unknown_int
        data.extend_from_slice(&0i16.to_be_bytes()); // 0 requirements
        data.extend_from_slice(&0i16.to_be_bytes()); // 0 rewards
        data.push(1); // completed: true
        data.push(0); // item_of_choice: false
        data.push(0); // repeatable: false

        let mut reader = PacketReader::new(&data);
        let quest = QuestData::deserialize(&mut reader).unwrap();

        assert!(quest.completed);
        assert!(!quest.repeatable);
        assert!(!quest.should_display()); // Should NOT display
    }

    #[test]
    fn test_quest_data_completed_repeatable() {
        let mut data = Vec::new();

        data.extend_from_slice(&make_string_bytes("quest_3"));
        data.extend_from_slice(&make_string_bytes("Repeatable Quest"));
        data.extend_from_slice(&make_string_bytes("Can do again"));
        data.extend_from_slice(&make_string_bytes(""));
        data.extend_from_slice(&0i32.to_be_bytes());
        data.extend_from_slice(&0i32.to_be_bytes());
        data.extend_from_slice(&0i16.to_be_bytes());
        data.extend_from_slice(&0i16.to_be_bytes());
        data.push(1); // completed: true
        data.push(0); // item_of_choice: false
        data.push(1); // repeatable: true

        let mut reader = PacketReader::new(&data);
        let quest = QuestData::deserialize(&mut reader).unwrap();

        assert!(quest.completed);
        assert!(quest.repeatable);
        assert!(quest.should_display()); // SHOULD display (repeatable)
    }

    #[test]
    fn test_quest_data_item_of_choice() {
        let mut data = Vec::new();

        data.extend_from_slice(&make_string_bytes("choice_quest"));
        data.extend_from_slice(&make_string_bytes("Choose Your Reward"));
        data.extend_from_slice(&make_string_bytes("Pick one"));
        data.extend_from_slice(&make_string_bytes(""));
        data.extend_from_slice(&2i32.to_be_bytes()); // category 2
        data.extend_from_slice(&0i32.to_be_bytes());
        // 1 requirement
        data.extend_from_slice(&1i16.to_be_bytes());
        data.extend_from_slice(&0xAAAAi32.to_be_bytes());
        // 3 reward choices
        data.extend_from_slice(&3i16.to_be_bytes());
        data.extend_from_slice(&0xBBBBi32.to_be_bytes());
        data.extend_from_slice(&0xCCCCi32.to_be_bytes());
        data.extend_from_slice(&0xDDDDi32.to_be_bytes());
        data.push(0); // completed: false
        data.push(1); // item_of_choice: true
        data.push(0); // repeatable: false

        let mut reader = PacketReader::new(&data);
        let quest = QuestData::deserialize(&mut reader).unwrap();

        assert_eq!(quest.name, "Choose Your Reward");
        assert_eq!(quest.requirements, vec![0xAAAA]);
        assert_eq!(quest.rewards, vec![0xBBBB, 0xCCCC, 0xDDDD]);
        assert!(quest.item_of_choice);
    }
}

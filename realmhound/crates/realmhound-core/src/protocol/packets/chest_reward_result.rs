//! ChestRewardResult packet implementation.
//!
//! Received with the final contents granted from a chest reward.

use super::claim_rewards_info_prompt::read_i32_array;
use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ChestRewardResult packet (ID 172) - Incoming
#[derive(Debug, Clone)]
pub struct ChestRewardResultPacket {
    /// The item type ids granted (may include -1 for empty).
    pub contents: Vec<i32>,
}

impl RotmgPacket for ChestRewardResultPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let contents = read_i32_array(reader)?;
        Ok(Self { contents })
    }

    fn description(&self) -> String {
        format!("ChestRewardResult: items={:?}", self.contents)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_bytes(items: &[i32]) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&(items.len() as i16).to_be_bytes());
        for &i in items {
            data.extend_from_slice(&i.to_be_bytes());
        }
        data
    }

    #[test]
    fn test_deserialize() {
        let data = build_bytes(&[100, 200, -1]);
        let mut reader = PacketReader::new(&data);
        let packet = ChestRewardResultPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.contents, vec![100, 200, -1]);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_empty() {
        let data = build_bytes(&[]);
        let mut reader = PacketReader::new(&data);
        let packet = ChestRewardResultPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.contents.len(), 0);
    }
}

//! ClaimRewardsInfoPrompt packet implementation.
//!
//! Received to preview the contents of a claimable chest reward.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// ClaimRewardsInfoPrompt packet (ID 170) - Incoming
#[derive(Debug, Clone)]
pub struct ClaimRewardsInfoPromptPacket {
    /// The type of chest the rewards are for.
    pub chest_type: u8,
    /// The item type ids contained in the reward (may include -1 for empty).
    pub contents: Vec<i32>,
}

impl RotmgPacket for ClaimRewardsInfoPromptPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let chest_type = reader.read_byte()?;
        let contents = read_i32_array(reader)?;

        Ok(Self {
            chest_type,
            contents,
        })
    }

    fn description(&self) -> String {
        format!(
            "ClaimRewardsInfoPrompt: chest={} items={:?}",
            self.chest_type, self.contents
        )
    }
}

/// Read an i16-length-prefixed array of i32 values, bounding the length by the
/// number of bytes actually remaining (each element is 4 bytes).
pub(super) fn read_i32_array(reader: &mut PacketReader) -> io::Result<Vec<i32>> {
    let len = reader.read_i16()?;
    if len < 0 || (len as usize) > reader.remaining() / 4 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Invalid i32 array length: {}", len),
        ));
    }
    let len = len as usize;
    let mut contents = Vec::with_capacity(len);
    for _ in 0..len {
        contents.push(reader.read_i32()?);
    }
    Ok(contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_bytes(chest_type: u8, items: &[i32]) -> Vec<u8> {
        let mut data = Vec::new();
        data.push(chest_type);
        data.extend_from_slice(&(items.len() as i16).to_be_bytes());
        for &i in items {
            data.extend_from_slice(&i.to_be_bytes());
        }
        data
    }

    #[test]
    fn test_deserialize() {
        let data = build_bytes(3, &[1234, -1, 5678]);
        let mut reader = PacketReader::new(&data);
        let packet = ClaimRewardsInfoPromptPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.chest_type, 3);
        assert_eq!(packet.contents, vec![1234, -1, 5678]);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_empty_contents() {
        let data = build_bytes(0, &[]);
        let mut reader = PacketReader::new(&data);
        let packet = ClaimRewardsInfoPromptPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.contents.len(), 0);
    }

    #[test]
    fn test_oversized_length_rejected() {
        let mut data = Vec::new();
        data.push(0);
        data.extend_from_slice(&100i16.to_be_bytes()); // claims 100 i32s but none follow
        let mut reader = PacketReader::new(&data);
        assert!(ClaimRewardsInfoPromptPacket::deserialize(&mut reader).is_err());
    }
}

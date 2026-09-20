//! BuyResult packet implementation.
//!
//! Received in response to a `BuyPacket`.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// BuyResult packet (ID 22) - Incoming
#[derive(Debug, Clone)]
pub struct BuyResultPacket {
    /// The result code.
    pub result: i32,
    /// Result description string.
    pub result_string: String,
}

impl RotmgPacket for BuyResultPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let result = reader.read_i32()?;
        let result_string = reader.read_string()?;
        Ok(Self {
            result,
            result_string,
        })
    }

    fn description(&self) -> String {
        format!(
            "BuyResult: result={} msg={}",
            self.result, self.result_string
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&0i32.to_be_bytes()); // result
        let msg = "Success";
        data.extend_from_slice(&(msg.len() as u16).to_be_bytes());
        data.extend_from_slice(msg.as_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = BuyResultPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.result, 0);
        assert_eq!(packet.result_string, "Success");
        assert!(reader.is_fully_parsed());
    }
}

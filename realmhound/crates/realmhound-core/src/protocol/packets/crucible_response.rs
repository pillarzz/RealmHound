//! CrucibleResponse packet implementation.
//!
//! Crucible JSON string response.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// Maximum number of entries accepted in either crucible array. Each i16 length
/// prefix is bound-checked against this before allocation.
const MAX_CRUCIBLE_ENTRIES: i16 = 4096;

/// CrucibleResponse packet (ID 183) - Incoming
#[derive(Debug, Clone)]
pub struct CrucibleResponsePacket {
    /// The crucible ids.
    pub crucible_ids: Vec<i32>,
    /// The crucible JSON payloads.
    pub crucible_jsons: Vec<String>,
}

fn read_len(reader: &mut PacketReader) -> io::Result<usize> {
    let len = reader.read_i16()?;
    if len < 0 || len > MAX_CRUCIBLE_ENTRIES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Invalid crucible array length: {}", len),
        ));
    }
    Ok(len as usize)
}

impl RotmgPacket for CrucibleResponsePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let ids_len = read_len(reader)?;
        let mut crucible_ids = Vec::with_capacity(ids_len);
        for _ in 0..ids_len {
            crucible_ids.push(reader.read_i32()?);
        }

        let jsons_len = read_len(reader)?;
        let mut crucible_jsons = Vec::with_capacity(jsons_len);
        for _ in 0..jsons_len {
            crucible_jsons.push(reader.read_string()?);
        }

        Ok(Self {
            crucible_ids,
            crucible_jsons,
        })
    }

    fn description(&self) -> String {
        format!(
            "CrucibleResponse: ids={} jsons={}",
            self.crucible_ids.len(),
            self.crucible_jsons.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&2i16.to_be_bytes()); // ids len
        data.extend_from_slice(&11i32.to_be_bytes());
        data.extend_from_slice(&22i32.to_be_bytes());
        data.extend_from_slice(&1i16.to_be_bytes()); // jsons len
        let json = "{\"a\":1}";
        data.extend_from_slice(&(json.len() as u16).to_be_bytes());
        data.extend_from_slice(json.as_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = CrucibleResponsePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.crucible_ids, vec![11, 22]);
        assert_eq!(packet.crucible_jsons, vec![json.to_string()]);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_negative_length_rejected() {
        let mut data = Vec::new();
        data.extend_from_slice(&(-1i16).to_be_bytes());
        let mut reader = PacketReader::new(&data);
        assert!(CrucibleResponsePacket::deserialize(&mut reader).is_err());
    }
}

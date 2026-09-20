//! MovePacket implementation.
//!
//! Sent to acknowledge a `NewTickPacket`, and to notify the server of the
//! client's current position and time.
//!

use super::super::data::MoveRecord;
use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// MovePacket (ID 62) - Outgoing
#[derive(Debug, Clone)]
pub struct MovePacket {
    /// The tick id of the `NewTickPacket` which this is acknowledging.
    pub tick_id: i32,
    /// The server real time in milliseconds.
    pub time: i32,
    /// The move records of the client (may be empty).
    pub records: Vec<MoveRecord>,
}

impl RotmgPacket for MovePacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let tick_id = reader.read_i32()?;
        let time = reader.read_i32()?;
        let count = reader.read_i16()?;
        if count < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Negative move record count: {}", count),
            ));
        }
        let count = count as usize;
        // Each MoveRecord is 12 bytes (i32 + 2x f32); reject if larger than payload.
        if count.saturating_mul(12) > reader.remaining() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Move record count {} exceeds remaining bytes", count),
            ));
        }
        let mut records = Vec::with_capacity(count);
        for _ in 0..count {
            records.push(MoveRecord::deserialize(reader)?);
        }

        Ok(Self {
            tick_id,
            time,
            records,
        })
    }

    fn description(&self) -> String {
        format!(
            "Move: tickId={}, time={}, records={}",
            self.tick_id,
            self.time,
            self.records.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize() {
        let mut data = Vec::new();
        data.extend_from_slice(&7i32.to_be_bytes()); // tickId
        data.extend_from_slice(&12345i32.to_be_bytes()); // time
        data.extend_from_slice(&2i16.to_be_bytes()); // record count
                                                     // record 0
        data.extend_from_slice(&100i32.to_be_bytes());
        data.extend_from_slice(&1.5f32.to_be_bytes());
        data.extend_from_slice(&2.5f32.to_be_bytes());
        // record 1
        data.extend_from_slice(&200i32.to_be_bytes());
        data.extend_from_slice(&3.5f32.to_be_bytes());
        data.extend_from_slice(&4.5f32.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = MovePacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.tick_id, 7);
        assert_eq!(packet.time, 12345);
        assert_eq!(packet.records.len(), 2);
        assert_eq!(packet.records[0].time, 100);
        assert_eq!(packet.records[0].pos.x, 1.5);
        assert_eq!(packet.records[1].pos.y, 4.5);
        assert!(reader.is_fully_parsed());
    }

    #[test]
    fn test_empty_records() {
        let mut data = Vec::new();
        data.extend_from_slice(&1i32.to_be_bytes());
        data.extend_from_slice(&2i32.to_be_bytes());
        data.extend_from_slice(&0i16.to_be_bytes());

        let mut reader = PacketReader::new(&data);
        let packet = MovePacket::deserialize(&mut reader).unwrap();
        assert!(packet.records.is_empty());
        assert!(reader.is_fully_parsed());
    }
}

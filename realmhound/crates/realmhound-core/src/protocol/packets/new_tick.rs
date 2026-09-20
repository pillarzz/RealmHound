//! NewTickPacket implementation.
//!
//! Received each game tick with status updates for all visible objects.
//! This is where we get continuous stat updates like player name, fame, etc.

use super::traits::RotmgPacket;
use super::PacketReader;
use crate::protocol::data::ObjectStatusData;
use std::io;

/// NewTickPacket (ID 10) - Incoming
///
/// Sent by server each game tick (~200ms).
/// Contains status updates for all visible objects including the player.
/// This is where we get continuous stat updates (name, fame, loot timers, etc.).
#[derive(Debug, Clone)]
pub struct NewTickPacket {
    /// Tick ID (incrementing counter)
    pub tick_id: i32,
    /// Time since last tick in milliseconds
    pub tick_time: i32,
    /// Server real time in milliseconds
    pub server_realtime_ms: u32,
    /// Server last RTT in milliseconds
    pub server_last_rtt_ms: u16,
    /// Status updates for visible objects
    pub statuses: Vec<ObjectStatusData>,
    /// Unknown new field (added after the reference implementation)
    pub unknown_byte: u8,
}

impl RotmgPacket for NewTickPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let tick_id = reader.read_i32()?;
        let tick_time = reader.read_i32()?;
        let server_realtime_ms = reader.read_u32()?;
        let server_last_rtt_ms = reader.read_u16()?;

        // Read status array - count is a readShort(), not a compressed int
        let status_count = reader.read_i16()?;

        // Sanity check: negative or absurdly large counts indicate garbage data (cipher not synced)
        if status_count < 0 || status_count > 1000 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Invalid status count: {} (likely cipher not synced)",
                    status_count
                ),
            ));
        }
        let status_count = status_count as usize;

        let mut statuses = Vec::with_capacity(status_count);
        for _ in 0..status_count {
            statuses.push(ObjectStatusData::deserialize(reader)?);
        }

        // New field added after the reference implementation - only present if there's remaining data
        let unknown_byte = if reader.remaining() > 0 {
            reader.read_byte()?
        } else {
            0
        };

        Ok(Self {
            tick_id,
            tick_time,
            server_realtime_ms,
            server_last_rtt_ms,
            statuses,
            unknown_byte,
        })
    }

    fn description(&self) -> String {
        format!(
            "NewTick: tick={}, time={}ms, {} statuses",
            self.tick_id,
            self.tick_time,
            self.statuses.len()
        )
    }
}

impl NewTickPacket {
    /// Find status for a specific object ID.
    pub fn find_status(&self, object_id: i32) -> Option<&ObjectStatusData> {
        self.statuses.iter().find(|s| s.object_id == object_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_tick_empty_with_trailing_byte() {
        // NewTick with 0 statuses AND the optional trailing byte present
        let mut data = Vec::new();

        // tick_id: 123
        data.extend_from_slice(&123i32.to_be_bytes());
        // tick_time: 200
        data.extend_from_slice(&200i32.to_be_bytes());
        // server_realtime_ms: 1000000
        data.extend_from_slice(&1000000u32.to_be_bytes());
        // server_last_rtt_ms: 50
        data.extend_from_slice(&50u16.to_be_bytes());
        // statuses: empty array
        data.extend_from_slice(&0i16.to_be_bytes());
        // unknown_byte: 42 (optional trailing byte present)
        data.push(42);

        let mut reader = PacketReader::new(&data);
        let packet = NewTickPacket::deserialize(&mut reader).unwrap();

        assert_eq!(packet.tick_id, 123);
        assert_eq!(packet.tick_time, 200);
        assert_eq!(packet.server_realtime_ms, 1000000);
        assert_eq!(packet.server_last_rtt_ms, 50);
        assert!(packet.statuses.is_empty());
        assert_eq!(packet.unknown_byte, 42);
    }

    /// Regression test for loot tracking bug (Jan 2026)
    ///
    /// When standing in vault with no nearby entities, server sends NewTick
    /// packets with 0 statuses and NO trailing byte - exactly 16 bytes.
    /// Previously, we always tried to read the trailing byte, causing
    /// "not enough bytes for u8" errors which broke loot tracking entirely
    /// (on_tick was never called because NewTick parsing failed).
    #[test]
    fn test_new_tick_without_trailing_byte_regression() {
        // NewTick with 0 statuses and NO trailing byte (exactly 16 bytes)
        // This is what the server sends when standing still in vault
        let mut data = Vec::new();

        // tick_id: 456
        data.extend_from_slice(&456i32.to_be_bytes());
        // tick_time: 200
        data.extend_from_slice(&200i32.to_be_bytes());
        // server_realtime_ms: 2000000
        data.extend_from_slice(&2000000u32.to_be_bytes());
        // server_last_rtt_ms: 30
        data.extend_from_slice(&30u16.to_be_bytes());
        // statuses: empty array (0 count)
        data.extend_from_slice(&0i16.to_be_bytes());
        // NO trailing byte - packet ends here at exactly 16 bytes

        assert_eq!(data.len(), 16, "Packet should be exactly 16 bytes");

        let mut reader = PacketReader::new(&data);
        let packet = NewTickPacket::deserialize(&mut reader);

        // This MUST succeed - previously it failed with "not enough bytes for u8"
        assert!(
            packet.is_ok(),
            "NewTick without trailing byte must parse successfully"
        );

        let packet = packet.unwrap();
        assert_eq!(packet.tick_id, 456);
        assert_eq!(packet.tick_time, 200);
        assert!(packet.statuses.is_empty());
        assert_eq!(
            packet.unknown_byte, 0,
            "Missing trailing byte should default to 0"
        );
    }

    #[test]
    fn test_new_tick_unknown_78_string_preserves_stat_alignment() {
        let mut data = Vec::new();
        data.extend_from_slice(&1i32.to_be_bytes());
        data.extend_from_slice(&200i32.to_be_bytes());
        data.extend_from_slice(&1_000u32.to_be_bytes());
        data.extend_from_slice(&50u16.to_be_bytes());
        data.extend_from_slice(&1i16.to_be_bytes());

        data.push(1);
        data.extend_from_slice(&0.0f32.to_be_bytes());
        data.extend_from_slice(&0.0f32.to_be_bytes());
        data.push(2);

        data.push(78);
        data.extend_from_slice(&17u16.to_be_bytes());
        data.extend_from_slice(b"Player01|Player02");
        data.push(0);

        data.push(1);
        data.extend_from_slice(&[0xBB, 0x01]);
        data.push(0x41);

        let mut reader = PacketReader::new(&data);
        let packet = NewTickPacket::deserialize(&mut reader).unwrap();

        assert!(reader.is_empty());
        assert_eq!(packet.statuses[0].stats.len(), 2);
        assert_eq!(
            packet.statuses[0].stats[0].stat_type,
            crate::protocol::data::StatType::Unknown78
        );
        assert_eq!(
            packet.statuses[0].stats[0].string_stat_value.as_deref(),
            Some("Player01|Player02")
        );
        assert_eq!(packet.statuses[0].stats[1].stat_value, 123);
    }
}

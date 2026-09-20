//! RealmHeroesLeft packet implementation.
//!
//! Received to tell the client how many heroes are left in the current realm.

use super::traits::RotmgPacket;
use super::PacketReader;
use std::io;

/// RealmHeroesLeft packet (ID 84) - Incoming
///
/// Sent by server when realm heroes are killed. Contains the count of
/// remaining heroes in the realm.
#[derive(Debug, Clone)]
pub struct RealmHeroesLeftPacket {
    /// Number of heroes remaining in the realm
    pub heroes_left: i32,
}

impl RotmgPacket for RealmHeroesLeftPacket {
    fn deserialize(reader: &mut PacketReader) -> io::Result<Self> {
        let heroes_left = reader.read_i32()?;

        Ok(Self { heroes_left })
    }

    fn description(&self) -> String {
        format!("RealmHeroesLeft: {} heroes remaining", self.heroes_left)
    }
}

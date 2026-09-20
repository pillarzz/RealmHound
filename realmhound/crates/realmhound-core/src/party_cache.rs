//! In-memory cache of player class/skin/guild sightings for the Party panel.
//!
//! Whenever a player becomes visible in the game world (via an Update packet)
//! their class, skin, dye textures, and guild are recorded here so the Party
//! panel can show accurate icons/guild tags immediately.

use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct PlayerSighting {
    pub class_id: i32,
    pub skin_id: i32,
    pub tex1: u32,
    pub tex2: u32,
    pub guild: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct PartySightingCache {
    players: HashMap<String, PlayerSighting>,
}

impl PartySightingCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, name: &str) -> Option<&PlayerSighting> {
        self.players.get(&name.to_lowercase())
    }

    pub fn insert(&mut self, name: &str, sighting: PlayerSighting) {
        self.players.insert(name.to_lowercase(), sighting);
    }
}

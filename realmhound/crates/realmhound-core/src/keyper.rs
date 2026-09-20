//! Detection helpers for the seasonal "The Keyper" realm event.
//!
//! The Keyper event runs in the realm: crystal towers appear (first wave ~33%
//! realm score, second wave ~66%), and once all towers are destroyed The Keyper
//! boss spawns. Deca gives us two realm-wide signals we can key off:
//!
//! - `#The Keyper` chat taunts (public, broadcast to the whole realm).
//! - The tower / boss entities appearing in `Update.new_objects` (only when
//!   near the local player).
//!
//! The first tower wave has NO chat taunt, so it can only be seen via the tower
//! entities; the second wave and both Keyper spawns each have a distinct taunt.

/// The Keyper boss object type (`ObjectID.list` id 19246).
pub const KEYPER_BOSS_ID: i32 = 19246;

/// The five crystal tower object types that make up a Keyper tower wave
/// (Amethyst / Sapphire / Emerald / Ruby / Topaz).
pub const KEYPER_TOWER_IDS: [i32; 5] = [19230, 19231, 19232, 19233, 19234];

/// A representative tower sprite id used for the Live Feed "towers appeared"
/// entry (Ruby Tower).
pub const KEYPER_TOWER_SPRITE_ID: i32 = 19233;

/// Which Keyper-event cue a chat taunt corresponds to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyperCue {
    /// The Keyper boss just spawned (all towers destroyed).
    KeyperSpawned,
    /// A crystal tower wave just appeared.
    TowersAppeared,
}

/// Whether an object type is one of the Keyper's crystal towers.
pub fn is_keyper_tower(object_type: i32) -> bool {
    KEYPER_TOWER_IDS.contains(&object_type)
}

/// Classify a chat taunt from `#The Keyper` into a Keyper-event cue.
///
/// `name` is the raw sender name (e.g. `"#The Keyper"`); `text` is the message.
/// Matching is done on lowercase substrings so straight vs. curly apostrophes
/// and trailing metadata don't matter. Returns `None` for non-Keyper senders
/// and for the Keyper's ordinary in-fight banter.
pub fn classify_keyper_taunt(name: &str, text: &str) -> Option<KeyperCue> {
    if !name
        .trim_start_matches('#')
        .eq_ignore_ascii_case("The Keyper")
    {
        return None;
    }
    let t = text.to_lowercase();
    // Keyper spawn: "Hands off those crystals! I need them to scrounge up more
    // keys!" (1st) / "Wha- Again? REALLY?!" (2nd).
    if t.contains("hands off those crystals") || t.contains("again? really") {
        return Some(KeyperCue::KeyperSpawned);
    }
    // Tower respawn (~66%): "Ah, there we go! Let's see those lowlifes try to
    // take down my crystals this time!".
    if t.contains("there we go") && t.contains("crystals") {
        return Some(KeyperCue::TowersAppeared);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyper_spawn_taunts_classify() {
        assert_eq!(
            classify_keyper_taunt(
                "#The Keyper",
                "Hands off those crystals! I need them to scrounge up more keys!"
            ),
            Some(KeyperCue::KeyperSpawned)
        );
        assert_eq!(
            classify_keyper_taunt("#The Keyper", "Wha- Again? REALLY?!"),
            Some(KeyperCue::KeyperSpawned)
        );
    }

    #[test]
    fn tower_respawn_taunt_classifies() {
        assert_eq!(
            classify_keyper_taunt(
                "#The Keyper",
                "Ah, there we go! Let\u{2019}s see those lowlifes try to take down my crystals this time!"
            ),
            Some(KeyperCue::TowersAppeared)
        );
    }

    #[test]
    fn in_fight_banter_is_ignored() {
        for banter in [
            "Quit messing with my operation!",
            "Find your own portals, freeloaders!",
            "Ack, get away from me!",
            "Fine, FINE! Have your precious dungeons! But you will rue this day!",
        ] {
            assert_eq!(
                classify_keyper_taunt("#The Keyper", banter),
                None,
                "{banter}"
            );
        }
    }

    #[test]
    fn other_senders_are_ignored() {
        assert_eq!(
            classify_keyper_taunt("SomePlayer", "Hands off those crystals!"),
            None
        );
    }

    #[test]
    fn tower_ids_recognized() {
        assert!(is_keyper_tower(19230));
        assert!(is_keyper_tower(19234));
        assert!(!is_keyper_tower(KEYPER_BOSS_ID));
        assert!(!is_keyper_tower(0));
    }
}

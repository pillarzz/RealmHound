//! Extra chat-ping triggers: character-name mentions and user-defined words.
//!
//! The RealmShark-parity social sounds cover whispers, party and guild chat as a
//! whole. These two are opt-in extras that look at a message's *body*: one fires
//! when the local character name (IGN) is mentioned, the other when a word the
//! user configured appears. Both match whole words only, so a trigger of `abyss`
//! never fires on `abyssal`.

/// Longest trigger text accepted for one custom-chat variant. The settings
/// field enforces the same cap, so a stored variant can never exceed it.
pub const CUSTOM_CHAT_TEXT_MAX: usize = 50;

/// Longest comma-separated trigger field accepted for one custom-chat alert.
pub const CUSTOM_CHAT_TEXT_TOTAL_MAX: usize = 2000;

/// Maximum number of custom-chat alerts a user can configure.
pub const CUSTOM_CHAT_ALERTS_MAX: usize = 20;

/// Whether `c` can be part of a word for trigger matching. Everything else
/// (spaces, punctuation, quotes) counts as a boundary, so `Bob's` mentions
/// `Bob` while `Bobsled` does not.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// True if `text` contains `needle` as a whole word, ASCII-case-insensitively.
///
/// `needle` may be several words ("gm me"); the boundaries are then checked
/// around the whole phrase. An empty (or whitespace-only) needle never matches.
pub fn contains_word(text: &str, needle: &str) -> bool {
    let needle = needle.trim();
    if needle.is_empty() {
        return false;
    }
    let haystack = text.to_lowercase();
    let needle = needle.to_lowercase();

    let mut searched = 0;
    while let Some(offset) = haystack[searched..].find(&needle) {
        let start = searched + offset;
        let end = start + needle.len();
        let before_is_boundary = haystack[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !is_word_char(c));
        let after_is_boundary = haystack[end..]
            .chars()
            .next()
            .is_none_or(|c| !is_word_char(c));
        if before_is_boundary && after_is_boundary {
            return true;
        }
        // Advance one character past this occurrence's start so overlapping and
        // boundary-rejected matches are not skipped. `start` and the step stay on
        // character boundaries because the needle's first char is UTF-8 there.
        searched = start + haystack[start..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// Split a comma-separated trigger field into its non-empty variants.
pub fn split_trigger_words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| c == ',' || c == '\n' || c == '\r')
        .map(str::trim)
        .filter(|word| !word.is_empty())
}

/// True if `body` contains any variant in a comma-separated trigger field as a
/// whole word.
pub fn matches_any_trigger(body: &str, words: &str) -> bool {
    split_trigger_words(words).any(|word| contains_word(body, word))
}

/// Normalise a comma-separated trigger field: trim variants, cap each at
/// [`CUSTOM_CHAT_TEXT_MAX`] characters, remove case-insensitive duplicates, and
/// keep the joined field within [`CUSTOM_CHAT_TEXT_TOTAL_MAX`] characters.
pub fn normalize_trigger_words(text: &str) -> String {
    let mut seen: Vec<String> = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut used = 0usize;

    for raw in text.split(|c: char| c == ',' || c == '\n' || c == '\r') {
        let word: String = raw
            .trim()
            .chars()
            .take(CUSTOM_CHAT_TEXT_MAX)
            .collect::<String>();
        if word.is_empty() {
            continue;
        }
        let key = word.to_lowercase();
        if seen.iter().any(|existing| existing == &key) {
            continue;
        }
        let added = word.chars().count() + if words.is_empty() { 0 } else { 2 };
        if used + added > CUSTOM_CHAT_TEXT_TOTAL_MAX {
            break;
        }
        used += added;
        seen.push(key);
        words.push(word);
    }

    words.join(", ")
}

/// Which extra chat pings a message body matches. Pure text matching: callers
/// apply the sound toggles themselves (see `SoundSettings::custom_chat` and
/// `SoundSettings::ign_mention`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChatPings {
    /// The local character name was mentioned.
    pub ign_mention: bool,
    /// The configured custom chat trigger text appeared.
    pub custom_chat: bool,
}

/// Whether `body` is a guild presence notice about a member ("Lovens has come
/// online.", "Lovens has gone offline.") rather than a player writing something.
/// The notice names the member in the message body, which is not someone
/// mentioning them, so it must not raise the character-name ping.
pub fn is_presence_notice(body: &str) -> bool {
    let body = body.trim();
    let body = body.strip_suffix('.').unwrap_or(body).trim_end();
    let body = body.to_lowercase();
    [" has come online", " has gone offline"]
        .iter()
        .any(|suffix| body.ends_with(suffix))
}

impl ChatPings {
    /// True if either ping matched.
    pub fn any(&self) -> bool {
        self.ign_mention || self.custom_chat
    }
}

/// Extra pings for one chat message.
///
/// `body` must be the message text only -- the author and the whisper recipient
/// are deliberately not searched, so writing a message (or receiving one) never
/// triggers a mention by itself. `ign` is the local character name; `trigger` is
/// the configured custom chat text.
pub fn pings_for(body: &str, ign: Option<&str>, trigger: &str) -> ChatPings {
    ChatPings {
        // A guild presence notice names the member without anyone mentioning
        // them, so it never raises the character-name ping.
        ign_mention: !is_presence_notice(body) && ign.is_some_and(|name| contains_word(body, name)),
        custom_chat: contains_word(body, trigger),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_a_whole_word_anywhere_in_the_message() {
        assert!(contains_word("wanna do abyss?", "abyss"));
        assert!(contains_word("abyss", "abyss"));
        assert!(contains_word("(abyss)", "abyss"));
        assert!(contains_word("let's run abyss later", "abyss"));
    }

    #[test]
    fn does_not_match_a_substring_of_a_longer_word() {
        assert!(!contains_word("abyssal depths", "abyss"));
        assert!(!contains_word("grabyssal", "abyss"));
    }

    #[test]
    fn does_not_match_inside_a_hyphen_joined_or_underscored_word() {
        // A hyphen is a boundary, an underscore is not (it can be part of a name).
        assert!(contains_word("the abyss-run", "abyss"));
        assert!(!contains_word("abyss_run", "abyss"));
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(contains_word("ABYSS please", "abyss"));
        assert!(contains_word("abyss please", "ABYSS"));
        assert!(contains_word("mention Bob here", "bob"));
    }

    #[test]
    fn empty_trigger_never_matches() {
        assert!(!contains_word("anything at all", ""));
        assert!(!contains_word("anything at all", "   "));
        assert!(!contains_word("", "abyss"));
    }

    #[test]
    fn a_possessive_still_mentions_the_name() {
        assert!(contains_word("Bob's turn to tank", "Bob"));
        assert!(contains_word("that was bob, right?", "Bob"));
        // A longer word starting with the name is not a mention.
        assert!(!contains_word("Bobsled time", "Bob"));
    }

    #[test]
    fn multi_word_triggers_match_as_a_phrase_with_boundaries() {
        assert!(contains_word("gm me a key", "gm me"));
        assert!(!contains_word("gm meter reader", "gm me"));
        assert!(contains_word("can you gm me?", "GM ME"));
    }

    #[test]
    fn matching_handles_non_ascii_text_and_names() {
        assert!(contains_word("привет Bob как дела", "bob"));
        assert!(contains_word("café abyss", "abyss"));
        assert!(!contains_word("caféabyss", "abyss"));
        // A non-ASCII needle is matched the same way.
        assert!(contains_word("это Абисс, вперёд", "абисс"));
    }

    #[test]
    fn pings_report_mentions_and_custom_triggers_separately() {
        let pings = pings_for("Bob saw abyss loot", Some("Bob"), "abyss");
        assert!(pings.ign_mention && pings.custom_chat && pings.any());

        let pings = pings_for("nothing interesting", Some("Bob"), "abyss");
        assert!(!pings.ign_mention && !pings.custom_chat && !pings.any());
    }

    #[test]
    fn pings_ignore_the_author_and_recipient() {
        // The body is all that is searched: an outgoing message that names the
        // author does not mention them, because only the text is looked at.
        let pings = pings_for("i'll tank", Some("Bob"), "abyss");
        assert!(!pings.any());
    }

    #[test]
    fn pings_without_a_known_name_only_track_the_custom_trigger() {
        let pings = pings_for("Bob saw abyss loot", None, "abyss");
        assert!(!pings.ign_mention);
        assert!(pings.custom_chat);
    }

    #[test]
    fn pings_with_no_configured_trigger_only_track_mentions() {
        let pings = pings_for("Bob saw loot", Some("Bob"), "");
        assert!(pings.ign_mention);
        assert!(!pings.custom_chat);
    }

    #[test]
    fn guild_presence_notices_do_not_mention_the_member() {
        for body in [
            "Lovens has come online.",
            "Lovens has come online",
            "Lovens has gone offline.",
            "  Lovens has gone offline.  ",
        ] {
            let pings = pings_for(body, Some("Lovens"), "");
            assert!(
                !pings.ign_mention,
                "{body:?} is a presence notice, not a mention"
            );
        }
        assert!(is_presence_notice("Lovens has come online."));
        assert!(is_presence_notice("Lovens has gone offline"));
        // A player writing about someone coming online still mentions them.
        assert!(!is_presence_notice("did Bob come online?"));
        assert!(!is_presence_notice("Bob has come online before"));
        assert!(!is_presence_notice("online"));
        assert!(!is_presence_notice(""));
        let pings = pings_for("did Bob come online?", Some("Bob"), "");
        assert!(pings.ign_mention);
    }

    #[test]
    fn matches_any_comma_separated_trigger_variant() {
        assert!(matches_any_trigger("wanna do abyss?", "abyss, abby, aby"));
        assert!(matches_any_trigger("abby run?", "abyss, abby, aby"));
        assert!(matches_any_trigger("aby now", "abyss, abby, aby"));
        assert!(!matches_any_trigger("nothing here", "abyss, abby, aby"));
        assert!(!matches_any_trigger("abyssal", "abyss, abby, aby"));
    }

    #[test]
    fn normalize_trigger_words_trims_caps_and_deduplicates() {
        assert_eq!(
            normalize_trigger_words(" abyss,  ABYSS ,abby,,aby "),
            "abyss, abby, aby"
        );
        assert_eq!(
            normalize_trigger_words("abyss\nabby\r\naby"),
            "abyss, abby, aby"
        );
        let long = "x".repeat(CUSTOM_CHAT_TEXT_MAX + 20);
        assert_eq!(
            normalize_trigger_words(&long).chars().count(),
            CUSTOM_CHAT_TEXT_MAX
        );
    }

    #[test]
    fn normalize_trigger_words_respects_the_total_field_cap() {
        let words = (0..100)
            .map(|i| format!("word{i:03}"))
            .collect::<Vec<_>>()
            .join(", ");
        assert!(normalize_trigger_words(&words).chars().count() <= CUSTOM_CHAT_TEXT_TOTAL_MAX);
    }
}

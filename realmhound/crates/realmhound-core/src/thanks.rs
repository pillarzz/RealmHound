//! Custom "thank you" message templates for Live Feed key pops and area
//! unlocks.
//!
//! Templates are stored verbatim in
//! [`LiveFeedSettings`](crate::settings::LiveFeedSettings) with fixed
//! placeholders that the user can surround with their own text but never
//! remove: `[Name]` / `[Names]` for the player names and `[Rune]` for the rune
//! type on Lost Halls monument unlocks. A template that lost a required
//! placeholder (for example because `settings.json` was edited by hand) falls
//! back to the built-in default, so a copied message always thanks somebody.

/// Maximum length of a copied "thank you" message, matching the game's chat
/// limit.
pub const MAX_MESSAGE_LEN: usize = 128;

/// Placeholder replaced with the opener/popper IGN.
pub const NAME_TOKEN: &str = "[Name]";
/// Placeholder replaced with the comma-separated, de-duplicated popper list.
pub const NAMES_TOKEN: &str = "[Names]";
/// Placeholder replaced with the rune type (`sword rune`) or the plural
/// `runes` when more than one rune (or more than one player) contributed.
pub const RUNE_TOKEN: &str = "[Rune]";

/// Default key-pop message.
pub const DEFAULT_KEY_POP: &str = "Thanks [Name] for the key!";
/// Default Wine Cellar Incantation message.
pub const DEFAULT_INC: &str = "Thanks [Name] for the inc!";
/// Default Vial of Pure Darkness message.
pub const DEFAULT_VIAL: &str = "Thanks [Name] for the vial!";
/// Default Lost Halls rune message. `[Rune]` renders as `sword rune` for a lone
/// rune from a lone player and as `runes` otherwise.
pub const DEFAULT_RUNES: &str = "Thanks [Names] for the [Rune]!";

/// Longest player name the game allows (the name limit was raised to 15
/// characters).
pub const MAX_NAME_LEN: usize = 15;

/// Most distinct poppers a rune unlock can thank (Sword, Shield, Helmet).
pub const MAX_RUNE_NAMES: usize = 3;

/// Longest phrase substituted for [`RUNE_TOKEN`] (`helmet rune`).
pub const MAX_RUNE_PHRASE: &str = "helmet rune";

/// Plural phrase substituted for [`RUNE_TOKEN`] when more than one rune (or
/// more than one player) contributed.
const PLURAL_RUNE_PHRASE: &str = "runes";

/// Which unlock an area-unlock entry thanks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AreaUnlockKind {
    /// Wine Cellar Incantation.
    Inc,
    /// Vial of Pure Darkness.
    Vial,
    /// Lost Halls rune monuments, combined on Oryx's Sanctuary entry.
    Runes,
}

/// Split `template` into the segments around `tokens`, which must all appear in
/// order. Returns `tokens.len() + 1` segments, or `None` when a token is missing
/// or out of order.
pub fn split_segments<'a>(template: &'a str, tokens: &[&str]) -> Option<Vec<&'a str>> {
    let mut rest = template;
    let mut segments = Vec::with_capacity(tokens.len() + 1);
    for token in tokens {
        let idx = rest.find(token)?;
        segments.push(&rest[..idx]);
        rest = &rest[idx + token.len()..];
    }
    segments.push(rest);
    Some(segments)
}

/// Join player names, dropping blanks and case-insensitive duplicates while
/// preserving the first spelling and the original order.
pub fn join_names<'a>(names: impl IntoIterator<Item = &'a str>) -> String {
    let mut unique: Vec<&str> = Vec::new();
    for name in names {
        let name = name.trim();
        if name.is_empty() || unique.iter().any(|seen| seen.eq_ignore_ascii_case(name)) {
            continue;
        }
        unique.push(name);
    }
    unique.join(", ")
}

/// Longest substitution for `[Names]`: three names of [`MAX_NAME_LEN`]
/// characters joined with `, `.
fn worst_names_len() -> usize {
    MAX_NAME_LEN * MAX_RUNE_NAMES + 2 * (MAX_RUNE_NAMES - 1)
}

/// Worst-case extra characters the placeholders in `tokens` can add once the
/// real names and rune type are substituted.
///
/// `[Names]` and `[Rune]` are correlated: three names always render the plural
/// `runes`, while the longest singular phrase (`helmet rune`) only appears with
/// a single name, so the runes case is sized on the longer of the two, not on
/// both maxima at once.
pub fn worst_case_expansion(tokens: &[&str]) -> usize {
    let placeholders: usize = tokens.iter().map(|token| token.chars().count()).sum();
    let has_names = tokens.contains(&NAMES_TOKEN);
    let has_rune = tokens.contains(&RUNE_TOKEN);

    let worst_substitution = if has_names && has_rune {
        (worst_names_len() + PLURAL_RUNE_PHRASE.chars().count())
            .max(MAX_NAME_LEN + MAX_RUNE_PHRASE.chars().count())
    } else {
        tokens
            .iter()
            .map(|token| match *token {
                NAME_TOKEN => MAX_NAME_LEN,
                NAMES_TOKEN => worst_names_len(),
                RUNE_TOKEN => MAX_RUNE_PHRASE.chars().count(),
                other => other.chars().count(),
            })
            .sum()
    };

    worst_substitution.saturating_sub(placeholders)
}

/// How many editable characters a template with `tokens` may use while the
/// assembled message still fits [`MAX_MESSAGE_LEN`] with the longest possible
/// names and rune type.
pub fn editable_budget(tokens: &[&str]) -> usize {
    let placeholders: usize = tokens.iter().map(|token| token.chars().count()).sum();
    MAX_MESSAGE_LEN
        .saturating_sub(placeholders)
        .saturating_sub(worst_case_expansion(tokens))
}

/// Worst-case assembled length of a template whose editable text is
/// `editable_chars` characters long and contains `tokens`.
pub fn worst_case_len(editable_chars: usize, tokens: &[&str]) -> usize {
    let placeholders: usize = tokens.iter().map(|token| token.chars().count()).sum();
    editable_chars + placeholders + worst_case_expansion(tokens)
}

/// Fill a single-name template, falling back to `default` when the template lost
/// its `[Name]` placeholder. Returns `None` when there is nobody to thank.
pub fn assemble_single(template: &str, default: &str, name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let template = if template.contains(NAME_TOKEN) {
        template
    } else {
        default
    };
    let (before, after) = split_once(template, NAME_TOKEN)?;
    Some(assemble_parts(before, name, after))
}

/// Fill the rune template. `runs` pairs each rune type (lowercase, e.g.
/// `sword`) with the player who popped it, for non-local poppers only.
///
/// `[Rune]` renders as `sword rune` when exactly one rune was popped by one
/// player and as `runes` otherwise, which reproduces the built-in phrasing
/// ("Thanks Greenmun for the sword rune!") without special-casing the default
/// template.
pub fn assemble_runes(template: &str, runs: &[(String, String)]) -> Option<String> {
    let names = join_names(runs.iter().map(|(_, name)| name.as_str()));
    if names.is_empty() {
        return None;
    }
    let template = if template.contains(NAMES_TOKEN) {
        template
    } else {
        DEFAULT_RUNES
    };
    let rune = if runs.len() == 1 {
        format!("{} rune", runs[0].0)
    } else {
        "runes".to_string()
    };
    let (before, after) = split_once(template, NAMES_TOKEN)?;
    let before = before.replace(RUNE_TOKEN, &rune);
    let after = after.replace(RUNE_TOKEN, &rune);
    Some(assemble_parts(&before, &names, &after))
}

/// Split `template` at `token` into the text before and after it.
fn split_once<'a>(template: &'a str, token: &str) -> Option<(&'a str, &'a str)> {
    let idx = template.find(token)?;
    Some((&template[..idx], &template[idx + token.len()..]))
}

/// Assemble `before + names + after`, clamped to [`MAX_MESSAGE_LEN`] characters.
///
/// The names are never truncated; the prefix keeps priority (it normally carries
/// the gratitude) and the tail of the suffix is dropped first.
fn assemble_parts(before: &str, names: &str, after: &str) -> String {
    let names_len = names.chars().count();
    let total = before.chars().count() + names_len + after.chars().count();
    if total <= MAX_MESSAGE_LEN {
        return format!("{before}{names}{after}");
    }

    let mut budget = MAX_MESSAGE_LEN.saturating_sub(names_len);
    let before_keep = before.chars().count().min(budget);
    budget -= before_keep;
    let after_keep = after.chars().count().min(budget);
    let before: String = before.chars().take(before_keep).collect();
    let after: String = after.chars().take(after_keep).collect();
    let mut text = format!("{before}{names}{after}");
    while text.ends_with(' ') {
        text.pop();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(rune: &str, name: &str) -> (String, String) {
        (rune.to_string(), name.to_string())
    }

    #[test]
    fn split_segments_requires_tokens_in_order() {
        assert_eq!(
            split_segments("Thanks [Names] for the [Rune]!", &[NAMES_TOKEN, RUNE_TOKEN]),
            Some(vec!["Thanks ", " for the ", "!"])
        );
        assert_eq!(split_segments("no tokens", &[NAME_TOKEN]), None);
        assert_eq!(
            split_segments("[Rune] [Names]", &[NAMES_TOKEN, RUNE_TOKEN]),
            None,
            "out-of-order tokens are rejected"
        );
    }

    #[test]
    fn join_names_dedupes_case_insensitively() {
        assert_eq!(join_names(["Alice", "Bob", "alice", " "]), "Alice, Bob");
        assert_eq!(join_names(std::iter::empty()), "");
    }

    #[test]
    fn worst_case_reserves_room_for_long_names_and_rune_phrases() {
        // One 15-character name replaces the 6-character [Name] token.
        assert_eq!(
            worst_case_expansion(&[NAME_TOKEN]),
            MAX_NAME_LEN - NAME_TOKEN.len()
        );
        // Three names + "runes" (54) beats one name + "helmet rune" (26).
        assert_eq!(worst_case_expansion(&[NAMES_TOKEN, RUNE_TOKEN]), 41);
        assert_eq!(editable_budget(&[NAME_TOKEN]), 113);
        assert_eq!(editable_budget(&[NAMES_TOKEN, RUNE_TOKEN]), 74);
        assert_eq!(worst_case_len(0, &[NAMES_TOKEN, RUNE_TOKEN]), 54);
    }

    #[test]
    fn worst_case_budget_keeps_single_name_messages_within_the_limit() {
        let template = format!("{}[Name]{}", "a".repeat(60), "b".repeat(53));
        let message = assemble_single(&template, DEFAULT_INC, "ABCDEFGHIJKLMNO").unwrap();
        assert_eq!(message.chars().count(), MAX_MESSAGE_LEN);
    }

    #[test]
    fn worst_case_budget_keeps_three_long_rune_names_within_the_limit() {
        let template = format!(
            "{}[Names]{}[Rune]{}",
            "a".repeat(30),
            "b".repeat(26),
            "c".repeat(18)
        );
        let runs = vec![
            (String::new(), "AAAAAAAAAAAAAAA".to_string()),
            (String::new(), "BBBBBBBBBBBBBBB".to_string()),
            (String::new(), "CCCCCCCCCCCCCCC".to_string()),
        ];
        let message = assemble_runes(&template, &runs).unwrap();
        assert_eq!(message.chars().count(), MAX_MESSAGE_LEN);
    }

    #[test]
    fn default_single_messages_match_the_shipped_phrases() {
        assert_eq!(
            assemble_single(DEFAULT_KEY_POP, DEFAULT_KEY_POP, "Lovens").as_deref(),
            Some("Thanks Lovens for the key!")
        );
        assert_eq!(
            assemble_single(DEFAULT_INC, DEFAULT_INC, "Lovens").as_deref(),
            Some("Thanks Lovens for the inc!")
        );
        assert_eq!(
            assemble_single(DEFAULT_VIAL, DEFAULT_VIAL, "Lovens").as_deref(),
            Some("Thanks Lovens for the vial!")
        );
    }

    #[test]
    fn custom_single_message_keeps_surrounding_text() {
        let template = "Ty [Name], much appreciated!";
        assert_eq!(
            assemble_single(template, DEFAULT_INC, "Lovens").as_deref(),
            Some("Ty Lovens, much appreciated!")
        );
    }

    #[test]
    fn missing_placeholder_falls_back_to_the_default() {
        assert_eq!(
            assemble_single("Thanks for the inc", DEFAULT_INC, "Lovens").as_deref(),
            Some("Thanks Lovens for the inc!")
        );
        assert_eq!(
            assemble_runes("Thanks for the runes", &[run("sword", "Lovens")]).as_deref(),
            Some("Thanks Lovens for the sword rune!")
        );
    }

    #[test]
    fn no_names_means_no_message() {
        assert_eq!(assemble_single(DEFAULT_INC, DEFAULT_INC, "  "), None);
        assert_eq!(assemble_runes(DEFAULT_RUNES, &[]), None);
    }

    #[test]
    fn default_runes_keep_the_singular_rune_phrase() {
        assert_eq!(
            assemble_runes(DEFAULT_RUNES, &[run("sword", "Greenmun")]).as_deref(),
            Some("Thanks Greenmun for the sword rune!")
        );
        assert_eq!(
            assemble_runes(
                DEFAULT_RUNES,
                &[run("sword", "Greenmun"), run("shield", "Greenmun")]
            )
            .as_deref(),
            Some("Thanks Greenmun for the runes!")
        );
        assert_eq!(
            assemble_runes(
                DEFAULT_RUNES,
                &[
                    run("sword", "Alice"),
                    run("shield", "Bob"),
                    run("helmet", "Alice"),
                ]
            )
            .as_deref(),
            Some("Thanks Alice, Bob for the runes!")
        );
    }

    #[test]
    fn custom_runes_template_keeps_rune_type_where_the_user_put_it() {
        let template = "Gg [Names]! Cheers for the [Rune].";
        assert_eq!(
            assemble_runes(template, &[run("shield", "Lovens")]).as_deref(),
            Some("Gg Lovens! Cheers for the shield rune.")
        );
        assert_eq!(
            assemble_runes(template, &[run("shield", "Lovens"), run("sword", "Bob")]).as_deref(),
            Some("Gg Lovens, Bob! Cheers for the runes.")
        );
    }

    #[test]
    fn assembled_messages_never_exceed_the_chat_limit() {
        let long = "x".repeat(200);
        let template = format!("{long}[Name]{long}");
        let message = assemble_single(&template, DEFAULT_INC, "Lovens").unwrap();
        assert!(
            message.chars().count() <= MAX_MESSAGE_LEN,
            "{} chars",
            message.chars().count()
        );
        assert!(message.contains("Lovens"), "the name is preserved");
        assert!(
            message.starts_with(&"x".repeat(MAX_MESSAGE_LEN - "Lovens".len())),
            "the prefix keeps priority from its start"
        );
        assert!(message.ends_with("Lovens"), "the suffix yields first");
    }

    #[test]
    fn multi_byte_text_is_not_cut_mid_character() {
        let template = format!("Merci [Name] {}", "é".repeat(200));
        let message = assemble_single(&template, DEFAULT_INC, "Lovens").unwrap();
        assert!(message.chars().count() <= MAX_MESSAGE_LEN);
        assert!(message.starts_with("Merci Lovens"));
    }
}

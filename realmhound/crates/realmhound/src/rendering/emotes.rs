//! Emote parsing and resolution for inline `<sprite name=X>` tags in chat messages.

use realmhound_core::assets::AssetManager;

const SPRITE_TAG_PREFIX: &str = "<sprite name=";

/// Inline sprite size for emotes in chat text.
pub(crate) const EMOTE_SPRITE_SIZE: f32 = 16.0;

/// Horizontal padding around inline emote sprites.
pub(crate) const EMOTE_PADDING: f32 = 4.0;

/// A segment of message text: either plain text or an emote sprite reference.
pub(crate) enum EmoteSegment<'a> {
    Text(&'a str),
    Emote(&'a str),
}

/// Returns true if the text contains at least one `<sprite name=...>` tag.
pub(crate) fn has_emote_tags(text: &str) -> bool {
    text.contains(SPRITE_TAG_PREFIX)
}

/// Parse message text containing `<sprite name=X>` tags into segments.
/// Handles quoted (`'name'` or `"name"`) and unquoted attribute values.
pub(crate) fn parse_segments(text: &str) -> Vec<EmoteSegment<'_>> {
    let mut segments = Vec::new();
    let mut rest = text;

    while let Some(start) = rest.find(SPRITE_TAG_PREFIX) {
        if start > 0 {
            segments.push(EmoteSegment::Text(&rest[..start]));
        }
        let after_tag = &rest[start + SPRITE_TAG_PREFIX.len()..];
        if let Some(end) = after_tag.find('>') {
            let name = after_tag[..end].trim_matches(|c| c == '\'' || c == '"');
            segments.push(EmoteSegment::Emote(name));
            rest = &after_tag[end + 1..];
        } else {
            segments.push(EmoteSegment::Text(&rest[start..]));
            rest = "";
            break;
        }
    }

    if !rest.is_empty() {
        segments.push(EmoteSegment::Text(rest));
    }

    segments
}

/// Resolve an emote sprite tag name to an object ID.
/// Handles underscores, case differences, and the " Emote" id_name suffix convention.
pub(crate) fn resolve_emote_id(asset_manager: &AssetManager, name: &str) -> Option<i32> {
    let humanized = name.replace('_', " ");
    let with_suffix = format!("{humanized} Emote");

    if let Some(id) = asset_manager.object_id_for_name(&with_suffix) {
        return Some(id);
    }

    // Title-case fallback for lowercase tag names
    let title_cased: String = with_suffix
        .split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().to_string() + c.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    if title_cased != with_suffix {
        if let Some(id) = asset_manager.object_id_for_name(&title_cased) {
            return Some(id);
        }
    }

    asset_manager.object_id_for_name(name)
}

/// Convert an emote name to a human-readable display string.
pub(crate) fn display_name(name: &str) -> String {
    name.replace('_', " ")
}

/// Fallback text shown when an emote sprite can't be rendered.
pub(crate) fn fallback_text(name: &str) -> String {
    format!(" [{}] ", display_name(name))
}

/// Strip `<sprite name=X>` tags from text, replacing them with `[Name]` for plain-text contexts.
pub(crate) fn strip_tags(text: &str) -> String {
    if !has_emote_tags(text) {
        return text.to_string();
    }
    let segments = parse_segments(text);
    let mut out = String::with_capacity(text.len());
    for seg in segments {
        match seg {
            EmoteSegment::Text(t) => out.push_str(t),
            EmoteSegment::Emote(name) => {
                out.push('[');
                out.push_str(&display_name(name));
                out.push(']');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic() {
        let text = "hello<sprite name=Bunshaker>world";
        let segments = parse_segments(text);
        assert_eq!(segments.len(), 3);
        assert!(matches!(segments[0], EmoteSegment::Text("hello")));
        assert!(matches!(segments[1], EmoteSegment::Emote("Bunshaker")));
        assert!(matches!(segments[2], EmoteSegment::Text("world")));
    }

    #[test]
    fn parse_multiple() {
        let text = "<sprite name=Bunshaker><sprite name=Bunny_Knife>";
        let segments = parse_segments(text);
        assert_eq!(segments.len(), 2);
        assert!(matches!(segments[0], EmoteSegment::Emote("Bunshaker")));
        assert!(matches!(segments[1], EmoteSegment::Emote("Bunny_Knife")));
    }

    #[test]
    fn parse_no_tags() {
        let text = "just normal text";
        let segments = parse_segments(text);
        assert_eq!(segments.len(), 1);
        assert!(matches!(
            segments[0],
            EmoteSegment::Text("just normal text")
        ));
    }

    #[test]
    fn parse_quoted_name() {
        let text = "<sprite name='bunny_butt'>";
        let segments = parse_segments(text);
        assert_eq!(segments.len(), 1);
        assert!(matches!(segments[0], EmoteSegment::Emote("bunny_butt")));
    }

    #[test]
    fn parse_double_quoted_name() {
        let text = "<sprite name=\"Exploding_Head\">";
        let segments = parse_segments(text);
        assert_eq!(segments.len(), 1);
        assert!(matches!(segments[0], EmoteSegment::Emote("Exploding_Head")));
    }

    #[test]
    fn strip_replaces_underscores() {
        let text = "hey<sprite name=Bunny_Knife>look";
        assert_eq!(strip_tags(text), "hey[Bunny Knife]look");
    }

    #[test]
    fn strip_no_tags_unchanged() {
        assert_eq!(strip_tags("hello world"), "hello world");
    }

    #[test]
    fn fallback_text_format() {
        assert_eq!(fallback_text("Bunny_Knife"), " [Bunny Knife] ");
    }
}

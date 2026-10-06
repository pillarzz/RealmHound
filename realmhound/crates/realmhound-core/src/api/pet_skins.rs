//! Parses the `account/getOwnedPetSkins` HTTP response.
//!
//! Pet-skin ownership never reaches the game socket, and char/list carries no
//! pet-skin list (unlike its `OwnedSkins` and `OwnedEmotes`), so this endpoint
//! is the only bulk source of the pet skins an account owns.
//!
//! The response wraps the pet-skin object ids in a single element, separated by
//! commas (the same shape as char/list's id lists):
//!
//! ```xml
//! <PetSkins>32846,50290,32783</PetSkins>
//! ```

use super::missions::extract_tag;

/// Element holding the owned pet-skin object ids.
const PET_SKINS_TAG: &str = "PetSkins";

/// Parse the pet-skin object ids the account owns.
///
/// Entries are comma-separated and may be padded with whitespace (including
/// newlines); blank and unparsable entries are skipped. Responses that carry no
/// list — an `<Error>` body, an empty body — yield no ids.
pub fn parse_owned_pet_skins(body: &str) -> Vec<i32> {
    extract_tag(body, PET_SKINS_TAG)
        .map(|inner| {
            inner
                .split(',')
                .filter_map(|entry| entry.trim().parse::<i32>().ok())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The documented response shape: one element, one line, comma-separated.
    #[test]
    fn parses_the_owned_pet_skin_list() {
        let body = "<PetSkins>32846,50290,32783,32717,6581,32828</PetSkins>";
        assert_eq!(
            parse_owned_pet_skins(body),
            vec![32846, 50290, 32783, 32717, 6581, 32828]
        );
    }

    /// The parser tolerates the pretty-printing the API uses for the sibling
    /// `account/ownedSkins` list, and skips blank/trailing entries.
    #[test]
    fn tolerates_whitespace_and_blank_entries() {
        let body = "<PetSkins>\n  30445, 19140,\n  24724 ,\n</PetSkins>";
        assert_eq!(parse_owned_pet_skins(body), vec![30445, 19140, 24724]);
    }

    #[test]
    fn error_bodies_yield_no_ids() {
        assert!(
            parse_owned_pet_skins("<Error>Error.noAccountCannotPurchaseSkin</Error>").is_empty()
        );
        assert!(parse_owned_pet_skins("<PetSkins></PetSkins>").is_empty());
        assert!(parse_owned_pet_skins("").is_empty());
        assert!(parse_owned_pet_skins("not xml").is_empty());
    }
}

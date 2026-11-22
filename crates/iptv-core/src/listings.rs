//! Localized wording for guide listings and group names.

use crate::guide::{GroupName, Listing};
use crate::i18n::{Language, translate};

/// Wording for the guide grid, program details and the recording draft.
pub fn listing_text(listing: Option<Listing>, language: &Language) -> String {
    match listing {
        Some(Listing::Title(title)) => title,
        Some(Listing::Gap) => translate(language, "No information").to_owned(),
        Some(Listing::Loading) => translate(language, "Loading…").to_owned(),
        None => String::new(),
    }
}

/// Wording for the groups column. Stored names pass through unchanged.
pub fn group_text(group: GroupName, language: &Language) -> String {
    match group {
        GroupName::Named(name) => name,
        GroupName::Ungrouped => translate(language, "Other").to_owned(),
        GroupName::Favorites => translate(language, "Favorites").to_owned(),
        GroupName::Recent => translate(language, "Recently watched").to_owned(),
        GroupName::AllChannels => translate(language, "All channels").to_owned(),
        GroupName::AllPlaylists => translate(language, "All playlists").to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{LANGUAGES, resolve};

    #[test]
    fn fixed_list_entries_use_the_catalog_in_every_language() {
        for language in LANGUAGES {
            for (name, source) in [
                (GroupName::Favorites, "Favorites"),
                (GroupName::Recent, "Recently watched"),
                (GroupName::AllChannels, "All channels"),
                (GroupName::AllPlaylists, "All playlists"),
                (GroupName::Ungrouped, "Other"),
            ] {
                assert_eq!(group_text(name, language), translate(language, source));
            }
            assert_eq!(
                group_text(GroupName::Named("Other".into()), language),
                "Other"
            );
        }
    }

    #[test]
    fn only_generated_listings_are_localized() {
        let language = resolve("ja", None);
        let text = |listing| listing_text(listing, language);
        assert_eq!(text(Some(Listing::Gap)), "情報なし");
        assert_eq!(
            text(Some(Listing::Loading)),
            translate(language, "Loading…")
        );
        assert_eq!(text(None), "");
        for title in ["No information", "Loading…", ""] {
            assert_eq!(text(Some(Listing::Title(title.into()))), title);
        }
    }
}

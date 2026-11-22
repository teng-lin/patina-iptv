//! Guide labels at the app's current language.

use iptv_core::guide::{GroupName, Listing};

pub fn listing(listing: Option<Listing>) -> String {
    iptv_presentation::listings::listing_text(listing, crate::i18n::language())
}

pub fn group(name: GroupName) -> String {
    // These four labels went through the shell's test pseudolocale before
    // listings returned typed values. Stored and guide-generated names do not.
    #[cfg(feature = "test-support")]
    {
        use crate::i18n::tr;
        match name {
            GroupName::Favorites => return tr("Favorites").to_owned(),
            GroupName::Recent => return tr("Recently watched").to_owned(),
            GroupName::AllChannels => return tr("All channels").to_owned(),
            GroupName::AllPlaylists => return tr("All playlists").to_owned(),
            GroupName::Named(_) | GroupName::Ungrouped => {}
        }
    }
    iptv_presentation::listings::group_text(name, crate::i18n::language())
}

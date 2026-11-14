//! The wizard's wording (iptv-rs's own).

use crate::i18n::{tr, trf};

use iptv_core::wizard::{
    FailureKind, Field, FieldError, ImportFailure, ImportProgress, ImportStage, PlaylistKind,
};

pub fn kind_title(kind: PlaylistKind) -> &'static str {
    match kind {
        PlaylistKind::M3uUrl => tr("Playlist link"),
        PlaylistKind::M3uFile => tr("Playlist file"),
        PlaylistKind::Xtream => tr("Xtream Codes login"),
        PlaylistKind::Stalker => tr("Stalker portal"),
    }
}

pub fn kind_detail(kind: PlaylistKind) -> &'static str {
    match kind {
        PlaylistKind::M3uUrl => tr("An M3U or M3U8 address starting with http:// or https://"),
        PlaylistKind::M3uFile => tr("An M3U file saved on this device"),
        PlaylistKind::Xtream => tr("A server address with a username and password"),
        PlaylistKind::Stalker => tr("A portal address and the MAC address it knows this device by"),
    }
}

pub fn label(field: Field) -> &'static str {
    match field {
        Field::PlaylistUrl => tr("Playlist link"),
        Field::PlaylistFile => tr("File"),
        Field::Server => tr("Server"),
        Field::Username => tr("Username"),
        Field::Password => tr("Password"),
        Field::PortalUrl => tr("Portal"),
        Field::Mac => tr("MAC address"),
        Field::Name => tr("Name"),
        Field::EpgUrl => tr("Guide link"),
    }
}

/// Shown in an empty field and as the text entry's placeholder.
pub fn placeholder(field: Field) -> &'static str {
    match field {
        Field::PlaylistUrl => "http://…/playlist.m3u",
        Field::PlaylistFile => tr("Choose a file…"),
        Field::Server => "http://server:port",
        Field::Username | Field::Password => tr("Required"),
        Field::PortalUrl => "http://portal/c/",
        Field::Mac => "00:1A:79:00:00:00",
        Field::Name => tr("Optional: how the playlist is listed"),
        Field::EpgUrl => tr("Optional: an XMLTV link (the provider's guide otherwise)"),
    }
}

pub fn field_error(error: FieldError) -> &'static str {
    match error {
        FieldError::Required => tr("Fill this in"),
        FieldError::ContainsWhitespace => tr("Remove the spaces"),
        FieldError::MissingScheme => tr("Start with http:// or https://"),
        FieldError::UnsupportedScheme => tr("Only http:// and https:// links work"),
        FieldError::InvalidHost => tr("Check the server name"),
        FieldError::InvalidPort => tr("The port is a number from 1 to 65535"),
        FieldError::InvalidMac => tr("A MAC address has 12 hexadecimal digits"),
    }
}

/// Why the last import did not add the playlist.
pub fn failure(kind: PlaylistKind, failure: ImportFailure) -> &'static str {
    if failure.field == Some(Field::EpgUrl) && failure.kind != FailureKind::Cancelled {
        return tr(
            "The guide link did not load. Correct it, or clear it to use the provider's guide.",
        );
    }
    let m3u = matches!(kind, PlaylistKind::M3uUrl | PlaylistKind::M3uFile);
    match failure.kind {
        FailureKind::Network if kind == PlaylistKind::M3uFile => {
            tr("The file could not be read. Choose it again.")
        }
        FailureKind::Network => {
            tr("No answer from the server. Check the address and the connection, then try again.")
        }
        FailureKind::Storage => tr(
            "Local storage could not be opened or updated. Restart the app with the latest version and try again.",
        ),
        FailureKind::Auth if m3u => tr("The server refused access to this playlist."),
        FailureKind::Auth => {
            tr("The provider refused these login details. Check them and try again.")
        }
        FailureKind::Parse if m3u => {
            tr("This is not a playlist the app can read. Check the address.")
        }
        FailureKind::Parse => {
            tr("The server did not answer as this type of service does. Check the address.")
        }
        FailureKind::Busy => {
            tr("Too many playlists are updating. Try again when an update finishes.")
        }
        FailureKind::EmptyPlaylist => tr("The playlist loaded but has nothing to watch in it."),
        FailureKind::Unsupported => {
            tr("This server is not a service of the chosen type. Try another type.")
        }
        FailureKind::Cancelled => {
            tr("Stopped. Your entries are kept; add the playlist again when ready.")
        }
    }
}

pub fn stage(progress: Option<ImportProgress>) -> String {
    let Some(p) = progress else {
        return tr("Starting…").to_owned();
    };
    let what = match p.stage {
        ImportStage::Connecting => tr("Connecting…"),
        ImportStage::Downloading => tr("Downloading…"),
        ImportStage::Parsing => tr("Reading the channels…"),
        ImportStage::Saving => tr("Saving…"),
    };
    match p.percent() {
        Some(pc) => trf(
            "{stage} {percent}%",
            &[("stage", what.into()), ("percent", pc.into())],
        ),
        None => what.to_owned(),
    }
}

pub fn counts(c: iptv_core::wizard::ImportCounts) -> String {
    let mut parts = vec![trf(
        "{channels, plural, one {# channel} other {# channels}} in \
         {groups, plural, one {# group} other {# groups}}",
        &[("channels", c.channels.into()), ("groups", c.groups.into())],
    )];
    if c.movies > 0 {
        parts.push(trf(
            "{count, plural, one {# film} other {# films}}",
            &[("count", c.movies.into())],
        ));
    }
    if c.series > 0 {
        parts.push(trf(
            "{count, plural, one {# series} other {# series}}",
            &[("count", c.series.into())],
        ));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use iptv_core::wizard::ImportCounts;

    #[test]
    fn every_failure_kind_has_its_own_message() {
        let kinds = [
            FailureKind::Network,
            FailureKind::Storage,
            FailureKind::Auth,
            FailureKind::Parse,
            FailureKind::EmptyPlaylist,
            FailureKind::Unsupported,
            FailureKind::Cancelled,
        ];
        let texts: std::collections::BTreeSet<_> = kinds
            .iter()
            .map(|k| failure(PlaylistKind::Xtream, ImportFailure::new(*k)))
            .collect();
        assert_eq!(texts.len(), kinds.len());
        let epg = ImportFailure {
            kind: FailureKind::Network,
            field: Some(Field::EpgUrl),
        };
        assert!(failure(PlaylistKind::M3uUrl, epg).contains("guide link"));
    }

    #[test]
    fn counts_read_naturally() {
        let c = ImportCounts {
            channels: 1,
            groups: 3,
            movies: 0,
            series: 2,
        };
        assert_eq!(counts(c), "1 channel in 3 groups · 2 series");
        let films = ImportCounts {
            channels: 2,
            groups: 1,
            movies: 1,
            series: 0,
        };
        assert_eq!(counts(films), "2 channels in 1 group · 1 film");
    }

    #[test]
    fn progress_and_field_errors_read_plainly() {
        assert_eq!(stage(None), "Starting…");
        let saving = ImportProgress {
            stage: ImportStage::Saving,
            done: 0,
            total: None,
        };
        assert_eq!(stage(Some(saving)), "Saving…");
        let half = ImportProgress {
            stage: ImportStage::Downloading,
            done: 50,
            total: Some(100),
        };
        assert_eq!(stage(Some(half)), "Downloading… 50%");
        let errors = [
            FieldError::Required,
            FieldError::ContainsWhitespace,
            FieldError::MissingScheme,
            FieldError::UnsupportedScheme,
            FieldError::InvalidHost,
            FieldError::InvalidPort,
            FieldError::InvalidMac,
        ];
        let texts: std::collections::BTreeSet<_> = errors.iter().map(|e| field_error(*e)).collect();
        assert_eq!(texts.len(), errors.len());
        // A file that cannot be read, and refused access to a playlist.
        let network = ImportFailure::new(FailureKind::Network);
        assert!(failure(PlaylistKind::M3uFile, network).contains("file"));
        let auth = ImportFailure::new(FailureKind::Auth);
        assert!(failure(PlaylistKind::M3uUrl, auth).contains("refused access"));
    }

    #[test]
    fn counts_follow_each_languages_plural_forms() {
        use crate::i18n::in_language;
        let c = ImportCounts {
            channels: 3,
            groups: 1,
            movies: 2,
            series: 5,
        };
        assert_eq!(counts(c), "3 channels in 1 group · 2 films · 5 series");
        assert_eq!(
            in_language("de", || counts(c)),
            "3 Sender in 1 Gruppe · 2 Filme · 5 Serien"
        );
        // Three forms, and the case the preposition asks for.
        assert_eq!(
            in_language("ru", || counts(c)),
            "3 канала в 1 группе · 2 фильма · 5 сериалов"
        );
        let many = ImportCounts {
            channels: 21,
            groups: 5,
            movies: 0,
            series: 0,
        };
        assert_eq!(in_language("ru", || counts(many)), "21 канал в 5 группах");
        // One form, and the language's own order.
        assert_eq!(
            in_language("ja", || counts(c)),
            "3チャンネル（1グループ） · 映画2本 · 5シリーズ"
        );
        let half = ImportProgress {
            stage: ImportStage::Downloading,
            done: 50,
            total: Some(100),
        };
        assert_eq!(
            in_language("de", || stage(Some(half))),
            "Herunterladen… 50 %"
        );
    }

    /// An import failure is told in the interface language, each kind in
    /// its own words: none falls back to English or to another kind's.
    #[test]
    fn every_failure_is_told_in_every_language() {
        use crate::i18n::in_language;
        let kinds = [
            FailureKind::Network,
            FailureKind::Storage,
            FailureKind::Auth,
            FailureKind::Parse,
            FailureKind::Busy,
            FailureKind::EmptyPlaylist,
            FailureKind::Unsupported,
            FailureKind::Cancelled,
        ];
        let told = |kind: PlaylistKind| -> Vec<&'static str> {
            kinds
                .iter()
                .map(|k| failure(kind, ImportFailure::new(*k)))
                .collect()
        };
        for kind in [
            PlaylistKind::M3uUrl,
            PlaylistKind::M3uFile,
            PlaylistKind::Xtream,
        ] {
            let english = told(kind);
            for language in iptv_core::i18n::LANGUAGES.iter().skip(1) {
                let texts = in_language(language.tag, || told(kind));
                let distinct: std::collections::BTreeSet<_> = texts.iter().collect();
                assert_eq!(distinct.len(), kinds.len(), "{}", language.tag);
                for (text, source) in texts.iter().zip(&english) {
                    assert_ne!(text, source, "{}: not translated", language.tag);
                }
            }
        }
        let auth = ImportFailure::new(FailureKind::Auth);
        assert_eq!(
            in_language("de", || failure(PlaylistKind::Xtream, auth)),
            "Der Anbieter hat diese Anmeldedaten abgelehnt. Prüfen und erneut versuchen."
        );
        let errors = [
            FieldError::Required,
            FieldError::ContainsWhitespace,
            FieldError::MissingScheme,
            FieldError::UnsupportedScheme,
            FieldError::InvalidHost,
            FieldError::InvalidPort,
            FieldError::InvalidMac,
        ];
        for language in iptv_core::i18n::LANGUAGES.iter().skip(1) {
            let texts: std::collections::BTreeSet<_> = in_language(language.tag, || {
                errors.map(field_error).into_iter().collect()
            });
            assert_eq!(texts.len(), errors.len(), "{}", language.tag);
            assert!(
                !texts.contains("Start with http:// or https://"),
                "{}",
                language.tag
            );
        }
    }
}

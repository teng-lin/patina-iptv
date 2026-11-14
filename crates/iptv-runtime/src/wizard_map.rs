//! The wizard's provider-neutral vocabulary ↔ the import pipeline's: a
//! [`PlaylistDraft`] becomes a [`Source`], import progress and typed
//! failures become the wizard's [`ImportProgress`] and [`ImportFailure`].
//! Pure functions, unit-tested below.

use iptv_core::wizard::{
    DraftSource, FailureKind, Field, ImportFailure, ImportProgress, ImportStage, PlaylistDraft,
};
use iptv_import::{FailureKind as Kind, ImportError, ParseProblem, Phase, Progress, Step};
use iptv_providers::model::{
    M3uSourceConfig, MacAddress, Source, SourceConfig, StalkerCredentials, XtreamCredentials,
};
use iptv_providers::{Secret, SecretUrl};

/// The source to import for `draft`, and the guide URL the user typed.
pub fn source_from_draft(draft: &PlaylistDraft) -> (Source, Option<SecretUrl>) {
    let m3u = |location: &str| {
        SourceConfig::M3u(M3uSourceConfig {
            location: SecretUrl::new(location),
            user_agent: None,
            referrer: None,
        })
    };
    let config = match &draft.source {
        DraftSource::M3uUrl { url } => m3u(url.expose()),
        DraftSource::M3uFile { path } => m3u(path),
        DraftSource::Xtream {
            server,
            username,
            password,
        } => SourceConfig::Xtream(XtreamCredentials {
            server_url: with_scheme(server.expose()),
            username: Secret::new(username.expose()),
            password: Secret::new(password.expose()),
            user_agent: None,
        }),
        DraftSource::Stalker { portal, mac } => SourceConfig::Stalker(StalkerCredentials {
            portal_url: with_scheme(portal.expose()),
            // The wizard normalised it already; parse again only to type it.
            mac: MacAddress::parse(mac.expose())
                .unwrap_or_else(|| MacAddress::from_stored(mac.expose())),
            serial: None,
            device_id: None,
            device_id2: None,
            signature: None,
            signature2: None,
            login: None,
            password: None,
        }),
    };
    let name = draft
        .name
        .clone()
        .unwrap_or_else(|| default_name(&draft.source));
    let epg = draft.epg_url.as_ref().map(|u| SecretUrl::new(u.expose()));
    (Source { name, config }, epg)
}

/// Server and portal addresses may omit the scheme; it is `https` then.
fn with_scheme(address: &str) -> String {
    if address.contains("://") {
        address.to_owned()
    } else {
        format!("https://{address}")
    }
}

/// A name when the user gave none: the server's host, or the file's name.
/// Never the path or query, which may carry credentials.
fn default_name(source: &DraftSource) -> String {
    let host = |url: &str| {
        let rest = url.split_once("://").map_or(url, |(_, r)| r);
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let host_port = authority.rsplit('@').next().unwrap_or("");
        let host = match host_port.strip_prefix('[') {
            Some(v6) => v6.split(']').next().unwrap_or(""),
            None => host_port.split(':').next().unwrap_or(""),
        };
        host.to_owned()
    };
    let name = match source {
        DraftSource::M3uUrl { url } => host(url.expose()),
        DraftSource::Xtream { server, .. } => host(server.expose()),
        DraftSource::Stalker { portal, .. } => host(portal.expose()),
        DraftSource::M3uFile { path } => path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("")
            .trim_end_matches(".m3u8")
            .trim_end_matches(".m3u")
            .to_owned(),
    };
    if name.is_empty() {
        "Playlist".to_owned()
    } else {
        name
    }
}

/// The wizard's coarse stage for one pipeline report.
pub fn progress_of(p: &Progress) -> ImportProgress {
    let (stage, done, total) = match p.phase {
        Phase::Connecting | Phase::Authenticating | Phase::Account => {
            (ImportStage::Connecting, 0, None)
        }
        Phase::Playlist | Phase::Guide(_) if p.bytes == 0 => (ImportStage::Connecting, 0, None),
        Phase::Playlist | Phase::Guide(_) => (ImportStage::Downloading, p.bytes, p.total_bytes),
        Phase::Groups(_) | Phase::Items(_) => (ImportStage::Downloading, p.items, None),
        Phase::Matching => (ImportStage::Parsing, p.items, None),
        Phase::Done => (ImportStage::Saving, 0, None),
        _ => (ImportStage::Parsing, p.items, None),
    };
    ImportProgress { stage, done, total }
}

/// The wizard's failure for a pipeline error. A guide failure points at
/// the EPG field; the wizard picks the field for the rest by kind.
pub fn failure_of(error: &ImportError) -> ImportFailure {
    let kind = match error {
        // Another kind of server than the chosen type speaks.
        ImportError::Parse {
            problem: ParseProblem::NotAPortal,
            ..
        } => FailureKind::Unsupported,
        // 5xx and rate limits are the server's trouble; other statuses
        // mean nothing usable is at that address.
        ImportError::HttpStatus { .. } if error.is_transient() => FailureKind::Network,
        ImportError::HttpStatus { .. } => FailureKind::Parse,
        _ => match error.kind() {
            Kind::Cancelled => FailureKind::Cancelled,
            // A portal error (`js.error`) is the server's trouble too.
            // The device could not keep it (disk full): as when the
            // playlist cannot be saved.
            Kind::Network | Kind::Server | Kind::Storage => FailureKind::Network,
            Kind::Auth => FailureKind::Auth,
            Kind::Empty => FailureKind::EmptyPlaylist,
            // Settings the provider client refuses (an address the
            // validators let through) are fixed in the same field.
            Kind::Config | Kind::Parse => FailureKind::Parse,
        },
    };
    ImportFailure {
        kind,
        field: matches!(error.step(), Some(Step::Guide(_))).then_some(Field::EpgUrl),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iptv_core::wizard::{SecretText, UrlText};
    use iptv_epg::FeedId;
    use iptv_import::{AuthProblem, FetchError, FetchErrorKind};
    use iptv_providers::model::ContentKind;

    fn draft(source: DraftSource) -> PlaylistDraft {
        PlaylistDraft {
            source,
            name: None,
            epg_url: None,
        }
    }

    #[test]
    fn m3u_url_and_file_become_m3u_locations() {
        let (s, epg) = source_from_draft(&PlaylistDraft {
            source: DraftSource::M3uUrl {
                url: UrlText::new("http://user:pw@iptv.example.test:8080/get.m3u?token=x"),
            },
            name: Some("Synthetic TV".into()),
            epg_url: Some(UrlText::new("http://epg.example.test/guide.xml.gz")),
        });
        assert_eq!(s.name, "Synthetic TV");
        let SourceConfig::M3u(c) = &s.config else {
            panic!("{s:?}")
        };
        assert_eq!(
            c.location.expose(),
            "http://user:pw@iptv.example.test:8080/get.m3u?token=x"
        );
        assert_eq!(
            epg.as_ref().map(SecretUrl::expose),
            Some("http://epg.example.test/guide.xml.gz")
        );

        let (s, epg) = source_from_draft(&draft(DraftSource::M3uFile {
            path: "/data/files/playlists/synthetic-list.m3u8".into(),
        }));
        assert_eq!(s.name, "synthetic-list");
        assert_eq!(epg, None);
        let SourceConfig::M3u(c) = &s.config else {
            panic!("{s:?}")
        };
        assert_eq!(
            c.location.expose(),
            "/data/files/playlists/synthetic-list.m3u8"
        );
    }

    #[test]
    fn default_names_are_the_host_without_credentials_or_path() {
        let (s, _) = source_from_draft(&draft(DraftSource::M3uUrl {
            url: UrlText::new("http://user:pw@iptv.example.test:8080/get.m3u?password=x"),
        }));
        assert_eq!(s.name, "iptv.example.test");
        let (s, _) = source_from_draft(&draft(DraftSource::Stalker {
            portal: UrlText::new("[2001:db8::1]:8080/c/"),
            mac: SecretText::new("00:1A:79:00:00:01"),
        }));
        assert_eq!(s.name, "2001:db8::1");
        let (s, _) = source_from_draft(&draft(DraftSource::M3uFile { path: "/".into() }));
        assert_eq!(s.name, "Playlist");
    }

    #[test]
    fn xtream_gets_credentials_and_a_scheme_when_missing() {
        let (s, _) = source_from_draft(&draft(DraftSource::Xtream {
            server: UrlText::new("panel.example.test:8080"),
            username: SecretText::new("synthetic-user"),
            password: SecretText::new(" pass with spaces "),
        }));
        assert_eq!(s.name, "panel.example.test");
        let SourceConfig::Xtream(c) = &s.config else {
            panic!("{s:?}")
        };
        assert_eq!(c.server_url, "https://panel.example.test:8080");
        assert_eq!(c.username.expose(), "synthetic-user");
        assert_eq!(c.password.expose(), " pass with spaces ");
        assert_eq!(c.user_agent, None);

        let (s, _) = source_from_draft(&draft(DraftSource::Xtream {
            server: UrlText::new("https://panel.example.test"),
            username: SecretText::new("u"),
            password: SecretText::new("p"),
        }));
        let SourceConfig::Xtream(c) = &s.config else {
            panic!("{s:?}")
        };
        assert_eq!(c.server_url, "https://panel.example.test");
    }

    #[test]
    fn stalker_gets_the_portal_and_the_mac() {
        let (s, _) = source_from_draft(&draft(DraftSource::Stalker {
            portal: UrlText::new("portal.example.test/c/"),
            mac: SecretText::new("00:1A:79:00:00:01"),
        }));
        let SourceConfig::Stalker(c) = &s.config else {
            panic!("{s:?}")
        };
        assert_eq!(c.portal_url, "https://portal.example.test/c/");
        assert_eq!(c.mac.expose(), "00:1A:79:00:00:01");
        assert_eq!((c.serial.clone(), c.login.clone()), (None, None));
    }

    #[test]
    fn debug_of_a_mapped_source_shows_no_secrets() {
        let (s, _) = source_from_draft(&draft(DraftSource::Xtream {
            server: UrlText::new("panel.example.test"),
            username: SecretText::new("synthetic-user"),
            password: SecretText::new("s3cret-pass"),
        }));
        let text = format!("{s:?}");
        assert!(!text.contains("synthetic-user") && !text.contains("s3cret-pass"));
    }

    fn net(step: Step) -> ImportError {
        ImportError::Network {
            step,
            error: FetchError::new(FetchErrorKind::Timeout, "timed out"),
        }
    }

    fn kind(e: ImportError) -> FailureKind {
        failure_of(&e).kind
    }

    #[test]
    fn failure_kinds_map_to_the_wizards() {
        use FailureKind::*;
        assert_eq!(kind(ImportError::Cancelled), Cancelled);
        assert_eq!(kind(net(Step::Playlist)), Network);
        let status = |status| ImportError::HttpStatus {
            step: Step::Playlist,
            status,
        };
        assert_eq!(kind(status(503)), Network);
        assert_eq!(kind(status(429)), Network);
        assert_eq!(kind(status(404)), Parse);
        assert_eq!(
            kind(ImportError::Auth {
                step: Step::AccountInfo,
                problem: AuthProblem::HttpStatus(401),
            }),
            Auth
        );
        assert_eq!(
            kind(ImportError::Auth {
                step: Step::Items(ContentKind::Live),
                problem: AuthProblem::XtreamRejected,
            }),
            Auth
        );
        let parse = |problem| ImportError::Parse {
            step: Step::Playlist,
            problem,
        };
        assert_eq!(kind(parse(ParseProblem::NotAPlaylist)), Parse);
        assert_eq!(kind(parse(ParseProblem::HlsManifest)), Parse);
        assert_eq!(kind(parse(ParseProblem::NotAPortal)), Unsupported);
        assert_eq!(
            kind(ImportError::Empty {
                step: Step::Playlist
            }),
            EmptyPlaylist
        );
        assert_eq!(kind(ImportError::InvalidConfig("x".into())), Parse);
    }

    #[test]
    fn only_guide_failures_point_at_a_field() {
        assert_eq!(failure_of(&net(Step::Playlist)).field, None);
        assert_eq!(
            failure_of(&net(Step::Guide(FeedId(1)))),
            ImportFailure {
                kind: FailureKind::Network,
                field: Some(Field::EpgUrl)
            }
        );
    }

    #[test]
    fn progress_maps_to_stages() {
        let p = |phase, bytes, items, total_bytes| Progress {
            phase,
            items,
            bytes,
            total_bytes,
            page: None,
        };
        let stage = |progress: &Progress| progress_of(progress).stage;
        assert_eq!(
            stage(&p(Phase::Authenticating, 0, 0, None)),
            ImportStage::Connecting
        );
        assert_eq!(
            stage(&p(Phase::Playlist, 0, 0, None)),
            ImportStage::Connecting
        );
        let dl = progress_of(&p(Phase::Playlist, 500, 3, Some(1000)));
        assert_eq!(dl.stage, ImportStage::Downloading);
        assert_eq!(dl.percent(), Some(50));
        let items = progress_of(&p(Phase::Items(ContentKind::Live), 9000, 120, None));
        assert_eq!((items.stage, items.done), (ImportStage::Downloading, 120));
        assert_eq!(stage(&p(Phase::Matching, 0, 0, None)), ImportStage::Parsing);
        assert_eq!(stage(&p(Phase::Done, 0, 0, None)), ImportStage::Saving);
    }
}

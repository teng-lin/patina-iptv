//! The playlists screen's wording (iptv-rs's own).

use crate::i18n::{tr, trf};

use crate::runtime::model::ImportErrorKind;
use iptv_core::wizard::{FailureKind, ImportFailure};
use iptv_providers::model::SourceKind;

use super::details::{Form, Param};
use super::state::{Act, Notice};
use crate::data::{Activity, Playlist};

pub fn kind(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::M3u => tr("M3U playlist"),
        SourceKind::Xtream => tr("Xtream Codes"),
        SourceKind::Stalker => tr("Stalker portal"),
    }
}

pub fn channels(n: u32) -> String {
    trf(
        "{count, plural, one {# channel} other {# channels}}",
        &[("count", n.into())],
    )
}

/// A time as the guide shows it: today's as `14:05`, others with the day.
fn when(at: i64, now: i64) -> String {
    crate::time::label(at, now)
}

/// Why a refresh recorded in the store failed (kind only).
pub fn stored_failure(kind: ImportErrorKind) -> &'static str {
    match kind {
        ImportErrorKind::Network => tr("no answer from the server"),
        ImportErrorKind::Timeout => tr("the server took too long to answer"),
        ImportErrorKind::HttpStatus => tr("the server turned the request down"),
        ImportErrorKind::InvalidBody => tr("the answer was not a playlist"),
        ImportErrorKind::InvalidConfig => tr("the playlist's details are incomplete"),
        ImportErrorKind::AccountRejected => tr("the provider refused the login"),
        ImportErrorKind::AuthFailed => tr("the portal ended the session"),
        ImportErrorKind::Blocked => tr("the portal does not accept this device"),
        ImportErrorKind::Cancelled => tr("it was stopped"),
        _ => tr("something went wrong"),
    }
}

/// Why this session's refresh failed.
pub fn failure(failure: ImportFailure) -> &'static str {
    match failure.kind {
        FailureKind::Storage => tr("local storage could not be opened or updated"),
        FailureKind::Network => tr("no answer from the server"),
        FailureKind::Auth => tr("access was refused"),
        FailureKind::Parse => tr("the answer was not a playlist"),
        FailureKind::EmptyPlaylist => tr("the playlist had nothing to watch in it"),
        FailureKind::Unsupported => tr("the server is not a service of this type"),
        FailureKind::Busy => tr("too many playlists are updating; try again shortly"),
        FailureKind::Cancelled => tr("it was stopped"),
    }
}

/// The row's second line: type and channels.
pub fn summary(p: &Playlist) -> String {
    let mut parts = vec![kind(p.kind).to_owned(), channels(p.channels)];
    if !p.policy.enabled {
        parts.push(tr("Disabled").to_owned());
    }
    parts.join(" · ")
}

/// The row's status: a refresh running or just finished, else what the
/// store has. A failure keeps the channels from the last success.
pub fn status(p: &Playlist, activity: Option<&Activity>, now: i64) -> (String, bool) {
    // A failure, and since when the channels still shown are.
    let failed = |failure: String| {
        let kept = p.updated_at.map(|at| {
            trf(
                "showing the channels from {when}",
                &[("when", (&when(at, now)).into())],
            )
        });
        match kept {
            Some(kept) => format!("{failure} · {kept}"),
            None => failure,
        }
    };
    match activity {
        Some(Activity::Running(progress)) => (crate::wizard_view::words::stage(*progress), false),
        Some(Activity::Checking(progress)) => (
            trf(
                "Checking the new details: {stage}",
                &[(
                    "stage",
                    (&crate::wizard_view::words::stage(*progress)).into(),
                )],
            ),
            false,
        ),
        Some(Activity::Rejected(f)) => (rejected(*f), true),
        // The store's kind is the more precise one, once recorded.
        Some(Activity::Done(Err(f))) => {
            let why = match p.failed {
                Some((kind, _)) if kind != ImportErrorKind::Other => stored_failure(kind),
                _ => failure(*f),
            };
            (
                failed(trf("Refresh failed: {reason}", &[("reason", why.into())])),
                true,
            )
        }
        _ => match (p.failed, p.updated_at) {
            (Some((kind, _)), _) => (
                failed(trf(
                    "Last refresh failed: {reason}",
                    &[("reason", stored_failure(kind).into())],
                )),
                true,
            ),
            (None, Some(at)) => (
                trf("Updated {when}", &[("when", (&when(at, now)).into())]),
                false,
            ),
            (None, None) => (String::new(), false),
        },
    }
}

/// A guide link for display: the address without user info, query values
/// hidden (accounts and tokens go there).
pub fn masked_link(link: &str) -> String {
    let (scheme, rest) = link.split_once("://").unwrap_or(("", link));
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let host = authority.rsplit('@').next().unwrap_or("");
    let (path, query) = tail.split_once('?').unwrap_or((tail, ""));
    let path = path.split('#').next().unwrap_or("");
    let mut shown = if scheme.is_empty() {
        format!("{host}{path}")
    } else {
        format!("{scheme}://{host}{path}")
    };
    if !query.is_empty() {
        let query = query.split('#').next().unwrap_or("");
        let keys: Vec<String> = query
            .split('&')
            .filter(|p| !p.is_empty())
            .map(|p| match p.split_once('=') {
                Some((k, _)) => format!("{k}=•••"),
                None => "•••".to_owned(),
            })
            .collect();
        shown.push('?');
        shown.push_str(&keys.join("&"));
    }
    shown
}

/// Where the playlist's guide comes from.
pub fn guide_line(p: &Playlist) -> String {
    let on: Vec<_> = p.guides.iter().filter(|g| g.enabled).collect();
    let from = match (on.as_slice(), p.guides.len()) {
        (_, 0) => tr("none").to_owned(),
        ([], _) => tr("all sources switched off").to_owned(),
        ([one], 1) if one.is_provider() => tr("the provider's own").to_owned(),
        ([one], _) => one
            .name
            .clone()
            .unwrap_or_else(|| masked_link(one.url.expose())),
        (on, all) if on.len() == all => trf(
            "{count, plural, one {# source} other {# sources}}",
            &[("count", all.into())],
        ),
        (on, all) => trf(
            "{on} of {all} sources",
            &[("on", on.len().into()), ("all", all.into())],
        ),
    };
    let guide = trf("Guide: {source}", &[("source", (&from).into())]);
    if p.guide_failed {
        format!("{guide} · {}", tr("did not load last time"))
    } else {
        guide
    }
}

/// New details that did not import.
pub fn rejected(f: ImportFailure) -> String {
    trf(
        "New details not saved: {reason}. The old ones stay.",
        &[("reason", failure(f).into())],
    )
}

pub fn act(act: Act, kind: Option<SourceKind>, shown: bool, running: bool) -> &'static str {
    match act {
        Act::Enabled => tr("Enable / disable playlist"),
        Act::RefreshInterval => tr("Automatic refresh"),
        Act::RefreshOnStart => tr("Refresh on startup"),
        Act::Channels => tr("Organize channels"),
        Act::CustomGroups => tr("Custom groups"),
        Act::Groups => tr("Organize groups"),
        Act::Show if shown => tr("On screen now: back to the guide"),
        Act::Show => tr("Show this playlist"),
        Act::Refresh if running => tr("Refreshing…"),
        Act::Refresh => tr("Refresh now"),
        Act::Rename => tr("Rename"),
        Act::Details => kind.map_or(tr("Provider details"), details_title),
        Act::UserAgent => tr("User-Agent"),
        Act::Guides => tr("Guide sources"),
        Act::Delete => tr("Delete"),
    }
}

pub fn notice(notice: Notice) -> &'static str {
    match notice {
        Notice::NameEmpty => tr("A playlist needs a name. The old one is kept."),
        Notice::Unchanged => tr("Nothing was changed."),
    }
}

/// The "Refresh all" row's status.
pub fn refreshing(running: usize) -> String {
    match running {
        0 => String::new(),
        n => trf(
            "{count, plural, one {Refreshing # playlist…} other {Refreshing # playlists…}}",
            &[("count", n.into())],
        ),
    }
}

/// The provider details a kind of playlist has.
pub fn details_title(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::M3u => tr("Playlist link"),
        SourceKind::Xtream => tr("Server and login"),
        SourceKind::Stalker => tr("Portal and MAC address"),
    }
}

pub fn param(p: Param) -> &'static str {
    crate::wizard_view::words::label(p.field())
}

/// Text entry's placeholder: a value that is not shown stays when the
/// field is left empty.
pub fn param_placeholder(p: Param, prefilled: bool) -> &'static str {
    if prefilled {
        crate::wizard_view::words::placeholder(p.field())
    } else {
        tr("Type the new value; left empty, the current one stays")
    }
}

/// A detail as the form shows it: logins and MAC addresses hidden, links
/// without accounts or tokens.
pub fn param_value(form: &Form, p: Param) -> String {
    let value = form.value(p);
    if p.secret() {
        "••••••••".to_owned()
    } else if p.is_link() {
        masked_link(&value)
    } else {
        value
    }
}

/// What deleting `p` takes away, and what the guide does next.
pub fn delete_consequences(p: &Playlist, next: Option<&str>, shown: bool) -> String {
    let gone = trf(
        "{count, plural, \
         one {Its # channel, its guide and your favourites from it will be gone from the app. \
         The playlist at your provider is not affected.} \
         other {Its # channels, its guide and your favourites from it will be gone from the app. \
         The playlist at your provider is not affected.}}",
        &[("count", p.channels.into())],
    );
    match (shown, next) {
        (true, Some(next)) => {
            let switches = trf("The guide switches to “{name}”.", &[("name", next.into())]);
            format!("{gone} {switches}")
        }
        (_, None) => format!("{gone} {}", tr("No playlists will be left.")),
        (false, Some(_)) => gone,
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::model::SourceId;

    use super::*;

    fn playlist() -> Playlist {
        Playlist {
            id: SourceId(1),
            name: "Synthetic".into(),
            kind: SourceKind::Xtream,
            channels: 1,
            updated_at: None,
            failed: None,
            guides: Vec::new(),
            guide_failed: false,
            user_agent: None,
            from_file: false,
            policy: Default::default(),
        }
    }

    #[test]
    fn links_never_show_credentials() {
        assert_eq!(
            masked_link("http://guide.example.test:8080/xmltv.php?username=u1&password=p1"),
            "http://guide.example.test:8080/xmltv.php?username=•••&password=•••"
        );
        assert_eq!(
            masked_link("https://user:secret@guide.example.test/epg.xml.gz#x"),
            "https://guide.example.test/epg.xml.gz"
        );
        assert_eq!(
            masked_link("http://guide.example.test/e.xml?token"),
            "http://guide.example.test/e.xml?•••"
        );
        assert_eq!(
            masked_link("http://guide.example.test"),
            "http://guide.example.test"
        );
    }

    #[test]
    fn statuses() {
        let mut p = playlist();
        assert_eq!(summary(&p), "Xtream Codes · 1 channel");
        assert_eq!(status(&p, None, 0), (String::new(), false));
        p.failed = Some((ImportErrorKind::Timeout, Some(5)));
        let (text, error) = status(&p, None, 0);
        assert!(error && text.contains("took too long"));
        let done = Activity::Done(Err(ImportFailure::new(FailureKind::Network)));
        // The kind the store recorded is the more precise one.
        let (text, error) = status(&p, Some(&done), 0);
        assert!(error && text.starts_with("Refresh failed: the server took too long"));
        p.failed = None;
        let (text, _) = status(&p, Some(&done), 0);
        assert!(text.starts_with("Refresh failed: no answer"));
        let (text, error) = status(&p, Some(&Activity::Running(None)), 0);
        assert!(!error && text == "Starting…");
    }

    #[test]
    fn the_guide_line_names_its_sources_and_a_failure() {
        use crate::data::guides;
        use iptv_providers::SecretUrl;
        let mut p = playlist();
        assert_eq!(guide_line(&p), "Guide: none");
        let provider = [SecretUrl::new(
            "http://panel.example.test/xmltv.php?password=p",
        )];
        p.guides = guides::sync_provider(&[], &provider);
        assert_eq!(guide_line(&p), "Guide: the provider's own");
        p.guides[0].enabled = false;
        p.guides.push(guides::user_source(
            guides::FIRST_USER_FEED,
            SecretUrl::new("http://guide.example.test/x.xml?token=t"),
            None,
        ));
        p.guide_failed = true;
        assert_eq!(
            guide_line(&p),
            "Guide: http://guide.example.test/x.xml?token=••• · did not load last time"
        );
        p.guides[0].enabled = true;
        p.guide_failed = false;
        assert_eq!(guide_line(&p), "Guide: 2 sources");
        p.guides[1].name = Some("Mine".into());
        p.guides[0].enabled = false;
        assert_eq!(guide_line(&p), "Guide: Mine");
        p.guides[1].enabled = false;
        assert_eq!(guide_line(&p), "Guide: all sources switched off");
    }

    #[test]
    fn every_failure_has_its_words() {
        use ImportErrorKind as K;
        let stored: Vec<_> = [
            K::Network,
            K::Timeout,
            K::HttpStatus,
            K::InvalidBody,
            K::InvalidConfig,
            K::AccountRejected,
            K::AuthFailed,
            K::Blocked,
            K::Cancelled,
            K::Other,
        ]
        .into_iter()
        .map(stored_failure)
        .collect();
        assert_eq!(stored[5], "the provider refused the login");
        assert_eq!(stored[9], "something went wrong");
        let mut all = stored.clone();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), stored.len(), "each kind its own words");
        let session: Vec<_> = [
            FailureKind::Network,
            FailureKind::Auth,
            FailureKind::Parse,
            FailureKind::EmptyPlaylist,
            FailureKind::Unsupported,
            FailureKind::Cancelled,
        ]
        .into_iter()
        .map(|k| failure(ImportFailure::new(k)))
        .collect();
        assert_eq!(session[1], "access was refused");
        assert_eq!(session[3], "the playlist had nothing to watch in it");
        assert_eq!(
            rejected(ImportFailure::new(FailureKind::Unsupported)),
            "New details not saved: the server is not a service of this type. The old ones stay."
        );
    }

    #[test]
    fn checks_rejections_and_updates_read_in_the_status() {
        let mut p = playlist();
        let checking = Activity::Checking(None);
        let (text, error) = status(&p, Some(&checking), 0);
        assert!(!error && text.starts_with("Checking the new details: "));
        let rejected = Activity::Rejected(ImportFailure::new(FailureKind::Auth));
        let (text, error) = status(&p, Some(&rejected), 0);
        assert!(error && text.contains("access was refused"));
        // A failure keeps the channels of the last success.
        p.updated_at = Some(1_800_000_000);
        p.failed = Some((ImportErrorKind::Other, None));
        let done = Activity::Done(Err(ImportFailure::new(FailureKind::Parse)));
        let (text, error) = status(&p, Some(&done), 1_800_000_000);
        assert!(error && text.starts_with("Refresh failed: the answer was not a playlist"));
        assert!(text.contains(" · showing the channels from "), "{text}");
        p.failed = None;
        let (text, error) = status(&p, None, 1_800_000_000);
        assert!(!error && text.starts_with("Updated "), "{text}");
    }

    #[test]
    fn actions_notices_and_details() {
        use super::super::details::Form;
        use iptv_providers::Secret;
        use iptv_providers::model::{Source, SourceConfig, XtreamCredentials};
        assert_eq!(kind(SourceKind::M3u), "M3U playlist");
        assert_eq!(kind(SourceKind::Stalker), "Stalker portal");
        assert_eq!(channels(2), "2 channels");
        assert_eq!(
            act(Act::Show, None, true, false),
            "On screen now: back to the guide"
        );
        assert_eq!(act(Act::Show, None, false, false), "Show this playlist");
        assert_eq!(act(Act::Refresh, None, false, true), "Refreshing…");
        assert_eq!(act(Act::Refresh, None, false, false), "Refresh now");
        assert_eq!(act(Act::Details, None, false, false), "Provider details");
        assert_eq!(
            act(Act::Details, Some(SourceKind::Stalker), false, false),
            "Portal and MAC address"
        );
        assert_eq!(act(Act::UserAgent, None, false, false), "User-Agent");
        assert_eq!(act(Act::Guides, None, false, false), "Guide sources");
        assert_eq!(act(Act::Delete, None, false, false), "Delete");
        assert_eq!(act(Act::Rename, None, false, false), "Rename");
        assert!(notice(Notice::NameEmpty).starts_with("A playlist needs a name"));
        assert_eq!(notice(Notice::Unchanged), "Nothing was changed.");
        assert_eq!(refreshing(0), "");
        assert_eq!(refreshing(1), "Refreshing 1 playlist…");
        assert_eq!(refreshing(3), "Refreshing 3 playlists…");
        assert_eq!(details_title(SourceKind::Xtream), "Server and login");
        assert_eq!(param(Param::Server), "Server");
        assert_eq!(param_placeholder(Param::Server, true), "http://server:port");
        assert!(param_placeholder(Param::Password, false).contains("current one stays"));
        // Logins never show; links without their accounts.
        let form = Form::new(
            SourceId(1),
            Source {
                name: "Synthetic".into(),
                config: SourceConfig::Xtream(XtreamCredentials {
                    server_url: "http://u:p@panel.example.test:8080/".into(),
                    username: Secret::new("synthetic-user"),
                    password: Secret::new("synthetic-pass"),
                    user_agent: None,
                }),
            },
        );
        assert_eq!(param_value(&form, Param::Username), "••••••••");
        assert_eq!(param_value(&form, Param::Password), "••••••••");
        assert_eq!(
            param_value(&form, Param::Server),
            "http://panel.example.test:8080/"
        );
        // A link without a scheme keeps its host and path.
        assert_eq!(
            masked_link("panel.example.test/x?a=1&&b"),
            "panel.example.test/x?a=•••&•••"
        );
    }

    #[test]
    fn deleting_states_its_consequences() {
        let p = playlist();
        let text = delete_consequences(&p, Some("Other"), true);
        assert!(text.contains("1 channel,") && text.contains("favourites"));
        assert!(text.contains("switches to “Other”"));
        assert!(delete_consequences(&p, None, true).contains("No playlists will be left"));
        assert!(!delete_consequences(&p, Some("Other"), false).contains("switches"));
    }

    #[test]
    fn rows_and_refresh_failures_follow_the_language() {
        use crate::i18n::in_language;
        let mut p = playlist();
        p.kind = SourceKind::M3u;
        p.channels = 3;
        p.policy.enabled = false;
        assert_eq!(summary(&p), "M3U playlist · 3 channels · Disabled");
        assert_eq!(
            in_language("de", || summary(&p)),
            "M3U-Wiedergabeliste · 3 Sender · Deaktiviert"
        );
        assert_eq!(
            in_language("ru", || summary(&p)),
            "Плейлист M3U · 3 канала · Выключен"
        );
        let done = Activity::Done(Err(ImportFailure::new(FailureKind::Network)));
        assert_eq!(
            in_language("de", || status(&p, Some(&done), 0)),
            (
                "Aktualisierung fehlgeschlagen: keine Antwort vom Server".to_owned(),
                true
            )
        );
        assert_eq!(
            in_language("ja", || status(&p, Some(&done), 0).0),
            "更新に失敗しました：サーバーから応答がありません"
        );
        p.failed = Some((ImportErrorKind::HttpStatus, Some(5)));
        assert_eq!(
            in_language("ru", || status(&p, None, 0).0),
            "Последнее обновление не удалось: сервер отклонил запрос"
        );
        assert_eq!(refreshing(0), "");
        assert_eq!(refreshing(1), "Refreshing 1 playlist…");
        assert_eq!(refreshing(4), "Refreshing 4 playlists…");
        assert_eq!(
            in_language("de", || refreshing(1)),
            "1 Wiedergabeliste wird aktualisiert…"
        );
        assert_eq!(
            in_language("de", || refreshing(4)),
            "4 Wiedergabelisten werden aktualisiert…"
        );
        assert_eq!(
            in_language("fr", || guide_line(&p)),
            "Guide : aucune source"
        );
        assert_eq!(
            in_language("de", || delete_consequences(&p, None, true)),
            in_language("de", || {
                format!(
                    "{} {}",
                    delete_consequences(&p, Some("Other"), false),
                    tr("No playlists will be left.")
                )
            })
        );
        assert!(
            in_language("de", || delete_consequences(&p, Some("Other"), true))
                .ends_with("Das Programm wechselt zu „Other“.")
        );
    }

    /// Why a refresh failed is told in the interface language, each kind
    /// in its own words.
    #[test]
    fn every_refresh_failure_is_told_in_every_language() {
        use crate::i18n::in_language;
        use ImportErrorKind as K;
        let stored = [
            K::Network,
            K::Timeout,
            K::HttpStatus,
            K::InvalidBody,
            K::InvalidConfig,
            K::AccountRejected,
            K::AuthFailed,
            K::Blocked,
            K::Cancelled,
            K::Other,
        ];
        let session = [
            FailureKind::Storage,
            FailureKind::Network,
            FailureKind::Auth,
            FailureKind::Parse,
            FailureKind::EmptyPlaylist,
            FailureKind::Unsupported,
            FailureKind::Busy,
            FailureKind::Cancelled,
        ];
        let told = || -> Vec<&'static str> {
            let stored = stored.iter().map(|kind| stored_failure(*kind));
            let session = session
                .iter()
                .map(|kind| failure(ImportFailure::new(*kind)));
            stored.chain(session).collect()
        };
        let english = told();
        for language in iptv_core::i18n::LANGUAGES.iter().skip(1) {
            let texts = in_language(language.tag, told);
            for (text, source) in texts.iter().zip(&english) {
                assert_ne!(text, source, "{}: not translated", language.tag);
            }
            let distinct = |texts: &[&str]| {
                texts
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
            };
            assert_eq!(
                distinct(&texts[..stored.len()]),
                stored.len(),
                "{}",
                language.tag
            );
            assert_eq!(
                distinct(&texts[stored.len()..]),
                session.len(),
                "{}",
                language.tag
            );
        }
    }
}

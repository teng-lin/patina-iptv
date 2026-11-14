//! The guide sources screen's wording (iptv-rs's own).

use crate::i18n::tr;

use crate::runtime::model::{GuideSource, ImportErrorKind};
use iptv_core::wizard::FieldError;

use super::state::{Act, EditKind, Notice};
use crate::data::stored::SourceLine;
use crate::playlists_view::masked_link;

/// A source's title: its name, else where it comes from.
pub fn title(s: &GuideSource) -> String {
    match (&s.name, s.provider_index) {
        (Some(name), _) => name.clone(),
        (None, Some(0)) => tr("The provider's guide").to_owned(),
        (None, Some(n)) => format!("The provider's guide {}", n + 1),
        (None, None) => masked_link(s.url.expose()),
    }
}

/// Second line: who added it and its address (accounts hidden).
pub fn origin(s: &GuideSource) -> String {
    let who = if s.is_provider() {
        tr("From the provider")
    } else {
        tr("Added by you")
    };
    format!("{who} · {}", masked_link(s.url.expose()))
}

/// A time shift: `No time shift`, `+1 h`, `−0:30 h`.
pub fn shift(minutes: i32) -> String {
    if minutes == 0 {
        return tr("No time shift").to_owned();
    }
    let sign = if minutes > 0 { "+" } else { "−" };
    let (h, m) = (minutes.abs() / 60, minutes.abs() % 60);
    if m == 0 {
        format!("Times {sign}{h} h")
    } else {
        format!("Times {sign}{h}:{m:02} h")
    }
}

/// Why a feed did not load (kind only).
pub fn failure(kind: ImportErrorKind) -> &'static str {
    match kind {
        ImportErrorKind::Network => "no answer from the server",
        ImportErrorKind::Timeout => "the server took too long",
        ImportErrorKind::HttpStatus => "the server turned the request down",
        ImportErrorKind::InvalidBody => "the answer was not guide data",
        ImportErrorKind::InvalidConfig => "the link is incomplete",
        ImportErrorKind::Cancelled => "it was stopped",
        _ => "something went wrong",
    }
}

fn count(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The source's status: off, its last result, or not downloaded yet.
/// `true` for a failure.
pub fn status(line: &SourceLine, when: impl Fn(i64) -> String) -> (String, bool) {
    let s = &line.source;
    let state = match &line.result {
        _ if !s.enabled => return ("Switched off: not downloaded".to_owned(), false),
        None => return (tr("Not downloaded yet").to_owned(), false),
        Some(r) => r,
    };
    let shift = if s.shift_minutes == 0 {
        String::new()
    } else {
        format!(" · {}", shift(s.shift_minutes))
    };
    match state.error {
        Some(kind) => (
            format!(
                "Failed {}: {}{shift}",
                when(state.attempted_at),
                failure(kind)
            ),
            true,
        ),
        None if state.channel_count == 0 && state.listed_count > 0 => (
            format!(
                "None of its {} is used here · updated {}{shift}",
                count(state.listed_count, "channel", "channels"),
                when(state.attempted_at)
            ),
            false,
        ),
        None => (
            format!(
                "{} used · {} · updated {}{shift}",
                count(state.channel_count, "channel", "channels"),
                count(state.programme_count, "programme", "programmes"),
                when(state.attempted_at)
            ),
            false,
        ),
    }
}

pub fn act(act: Act, s: &GuideSource) -> String {
    match act {
        Act::Switch if s.enabled => tr("Switch off").to_owned(),
        Act::Switch => tr("Switch on").to_owned(),
        Act::Up => tr("Move up (used first)").to_owned(),
        Act::Down => tr("Move down").to_owned(),
        Act::Shift => shift(s.shift_minutes),
        Act::Rename => tr("Name").to_owned(),
        Act::Relink => tr("Change the link").to_owned(),
        Act::Remove => tr("Remove").to_owned(),
    }
}

pub fn notice(notice: Notice) -> &'static str {
    match notice {
        Notice::Link(FieldError::Required) => "Type the guide's link (XMLTV).",
        Notice::Link(e) => crate::wizard_view::words::field_error(e),
        Notice::Duplicate => "This playlist has that guide already.",
    }
}

/// The text sheet's label and placeholder.
pub fn sheet(kind: &EditKind) -> (&'static str, &'static str) {
    match kind {
        EditKind::AddLink => (
            "New guide source: link (XMLTV)",
            "http://…/guide.xml or .xml.gz",
        ),
        EditKind::AddName { .. } => ("Its name (optional)", "Left empty, the link is shown"),
        EditKind::Rename(_) => (tr("Name"), "Left empty, where it comes from is shown"),
        EditKind::Relink(_) => ("New link (XMLTV)", "Left empty, the current one stays"),
    }
}

/// What removing a source does.
pub fn remove_consequences(s: &GuideSource) -> String {
    format!(
        "“{}” will no longer be downloaded. Its programmes go away at the next guide update.",
        title(s)
    )
}

#[cfg(test)]
mod tests {
    use iptv_library::FeedResult;
    use iptv_providers::SecretUrl;

    use super::*;
    use crate::data::guides;

    fn line(enabled: bool, result: Option<FeedResult>) -> SourceLine {
        let mut source = guides::user_source(
            guides::FIRST_USER_FEED,
            SecretUrl::new("http://guide.example.test/e.xml?token=t"),
            None,
        );
        source.enabled = enabled;
        source.shift_minutes = -90;
        SourceLine { source, result }
    }

    #[test]
    fn titles_and_shifts() {
        let l = line(true, None);
        assert_eq!(
            title(&l.source),
            "http://guide.example.test/e.xml?token=•••"
        );
        assert_eq!(
            origin(&l.source),
            "Added by you · http://guide.example.test/e.xml?token=•••"
        );
        let p = &guides::sync_provider(&[], &[SecretUrl::new("http://p.example.test/x")])[0];
        assert_eq!(title(p), "The provider's guide");
        assert_eq!(shift(0), "No time shift");
        assert_eq!(shift(120), "Times +2 h");
        assert_eq!(shift(-90), "Times −1:30 h");
    }

    #[test]
    fn names_actions_notices_and_sheets() {
        use iptv_core::wizard::FieldError;
        let mut l = line(true, None);
        // The provider's second guide, a named source.
        let urls = [
            SecretUrl::new("http://p.example.test/a"),
            SecretUrl::new("http://p.example.test/b"),
        ];
        let p = guides::sync_provider(&[], &urls);
        assert_eq!(title(&p[1]), "The provider's guide 2");
        assert_eq!(origin(&p[1]), "From the provider · http://p.example.test/b");
        l.source.name = Some("Synthetic Guide".into());
        assert_eq!(title(&l.source), "Synthetic Guide");
        assert_eq!(
            remove_consequences(&l.source),
            "“Synthetic Guide” will no longer be downloaded. \
             Its programmes go away at the next guide update."
        );
        assert_eq!(shift(30), "Times +0:30 h");
        assert_eq!(act(Act::Switch, &l.source), "Switch off");
        l.source.enabled = false;
        assert_eq!(act(Act::Switch, &l.source), "Switch on");
        assert_eq!(act(Act::Up, &l.source), "Move up (used first)");
        assert_eq!(act(Act::Down, &l.source), "Move down");
        assert_eq!(act(Act::Shift, &l.source), "Times −1:30 h");
        assert_eq!(act(Act::Rename, &l.source), "Name");
        assert_eq!(act(Act::Relink, &l.source), "Change the link");
        assert_eq!(act(Act::Remove, &l.source), "Remove");
        assert_eq!(
            notice(Notice::Link(FieldError::Required)),
            "Type the guide's link (XMLTV)."
        );
        assert_eq!(
            notice(Notice::Link(FieldError::MissingScheme)),
            "Start with http:// or https://"
        );
        assert_eq!(
            notice(Notice::Duplicate),
            "This playlist has that guide already."
        );
        assert_eq!(
            sheet(&EditKind::AddLink).0,
            "New guide source: link (XMLTV)"
        );
        let name = EditKind::AddName { url: String::new() };
        assert_eq!(sheet(&name).0, "Its name (optional)");
        let feed = guides::FIRST_USER_FEED;
        assert_eq!(sheet(&EditKind::Rename(feed)).0, "Name");
        assert_eq!(sheet(&EditKind::Relink(feed)).0, "New link (XMLTV)");
        let kinds = [
            ImportErrorKind::Network,
            ImportErrorKind::Timeout,
            ImportErrorKind::HttpStatus,
            ImportErrorKind::InvalidBody,
            ImportErrorKind::InvalidConfig,
            ImportErrorKind::Cancelled,
            ImportErrorKind::Other,
        ];
        let words: Vec<_> = kinds.into_iter().map(failure).collect();
        assert_eq!(words[0], "no answer from the server");
        assert_eq!(words[6], "something went wrong");
        let mut distinct = words.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), words.len());
    }

    #[test]
    fn the_status_says_how_its_last_download_went() {
        let when = |_| "14:05".to_owned();
        assert_eq!(
            status(&line(false, None), when).0,
            "Switched off: not downloaded"
        );
        assert_eq!(status(&line(true, None), when).0, "Not downloaded yet");
        let mut r = FeedResult {
            feed: guides::FIRST_USER_FEED,
            attempted_at: 0,
            error: None,
            channel_count: 1,
            programme_count: 240,
            listed_count: 50,
        };
        assert_eq!(
            status(&line(true, Some(r.clone())), when),
            (
                "1 channel used · 240 programmes · updated 14:05 · Times −1:30 h".to_owned(),
                false
            )
        );
        r.channel_count = 0;
        assert_eq!(
            status(&line(true, Some(r.clone())), when).0,
            "None of its 50 channels is used here · updated 14:05 · Times −1:30 h"
        );
        r.error = Some(ImportErrorKind::HttpStatus);
        let (text, failed) = status(&line(true, Some(r)), when);
        assert!(failed && text.starts_with("Failed 14:05: the server turned"));
    }
}

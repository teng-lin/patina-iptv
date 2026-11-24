//! Stable content and group ids.
//!
//! User customizations (favorites, order, hidden, renamed, blocked) live in
//! overlay tables keyed by these ids, while provider rows are replaced on
//! every refresh. An id must therefore depend only on what identifies the
//! item at the provider, never on list position or on volatile URL parts.
//! Ids are unique **within one source**; overlay keys are
//! `(source id, content id)`.
//!
//! # The rule
//!
//! 1. **Provider stream id exists** (Xtream `stream_id`/`series_id`/episode
//!    `id`, Stalker `id`): `"{source}:{kind}:{provider_id}"`, for example
//!    `xt:live:1234`, `xt:movie:1234`, `st:live:77`. The kind is included
//!    because panels reuse the same number for a live channel and a movie.
//!    Numeric ids are canonicalized (`"0042"` → `42`).
//! 2. **No provider id** (M3U entries):
//!    `"m3u:" + hex(xxh3_64(normalize_stream_url(url) + "\n" + normalize_name(name)))`.
//!    Normalization removes what changes between refreshes without changing
//!    the stream: scheme (`http`/`https`), letter case of the host, default
//!    ports, userinfo, fragment, the `|User-Agent=…` header suffix, volatile
//!    query keys (`token`, `auth`, `expires`, `hdnts`, …; see
//!    [`VOLATILE_QUERY_KEYS`]), query order, and — for Xtream-shaped paths
//!    (`/live/USER/PASS/ID.ext`, `/USER/PASS/ID`) — the credentials and the
//!    `.ts`/`.m3u8` extension. Names are trimmed, whitespace-collapsed and
//!    lowercased. Untitled M3U entries retain their historical URL-derived
//!    fallback as the hash input; their neutral display label is independent.
//! 3. **Duplicates** (two M3U entries with the same normalized URL and name)
//!    get `~2`, `~3`, … in file order, so every id in one import is unique.
//!
//! Consequences: renaming an M3U channel at the provider, or moving it to a
//! different stream path, yields a new id (the old customization stops
//! matching); a password change, token rotation, `http`→`https` move or a
//! `.ts`→`.m3u8` switch does not. Changing this rule changes persisted keys
//! and needs a data migration.
//!
//! Groups: `"{source}:{kind}-group:{provider category id}"`, or for M3U
//! `"m3u:group:" + hex(xxh3_64(normalize_name(title)))`.

use std::collections::HashMap;
use std::fmt;

use xxhash_rust::xxh3::xxh3_64;

use crate::model::{ContentKind, SourceKind};

/// Query keys dropped before hashing because they rotate between refreshes.
pub const VOLATILE_QUERY_KEYS: &[&str] = &[
    "token",
    "auth",
    "auth_token",
    "access_token",
    "hdnts",
    "hdnea",
    "wmsauthsign",
    "expires",
    "exp",
    "signature",
    "sig",
    "st",
    "e",
    "utc",
    "lutc",
    "username",
    "password",
];

/// Stable id of a channel, movie, series or episode within one source.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ContentId(String);

impl ContentId {
    /// Rule 1: an id from the provider's own stream id.
    pub fn from_provider(source: SourceKind, kind: ContentKind, provider_id: &str) -> Self {
        Self(format!(
            "{}:{}:{}",
            source.id_prefix(),
            kind.slug(),
            canonical_provider_id(provider_id)
        ))
    }

    /// Rule 2: an id for an M3U entry without a provider id.
    pub fn from_m3u(url: &str, name: &str) -> Self {
        let key = format!("{}\n{}", normalize_stream_url(url), normalize_name(name));
        Self(format!("m3u:{:016x}", xxh3_64(key.as_bytes())))
    }

    /// Re-creates an id read back from storage. No validation.
    pub fn from_stored(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ContentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Stable id of a group/category within one source.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct GroupId(String);

impl GroupId {
    pub fn from_provider(source: SourceKind, kind: ContentKind, provider_id: &str) -> Self {
        Self(format!(
            "{}:{}-group:{}",
            source.id_prefix(),
            kind.slug(),
            canonical_provider_id(provider_id)
        ))
    }

    pub fn from_m3u_title(title: &str) -> Self {
        Self(format!(
            "m3u:group:{:016x}",
            xxh3_64(normalize_name(title).as_bytes())
        ))
    }

    pub fn from_stored(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for GroupId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `" 0042 "` → `"42"`; non-numeric ids are only trimmed.
fn canonical_provider_id(id: &str) -> String {
    let id = id.trim();
    match id.parse::<u64>() {
        Ok(n) => n.to_string(),
        Err(_) => id.to_owned(),
    }
}

/// Trim, collapse internal whitespace, lowercase.
pub fn normalize_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for word in name.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.extend(word.chars().flat_map(char::to_lowercase));
    }
    out
}

/// The URL key used by rule 2. See the module docs for what is removed.
pub fn normalize_stream_url(raw: &str) -> String {
    let trimmed = raw.trim();
    let without_pipe = trimmed.split('|').next().unwrap_or("").trim();
    let Ok(url) = url::Url::parse(without_pipe) else {
        return without_pipe.to_lowercase();
    };
    let Some(host) = url.host_str() else {
        // Non-hierarchical (`rtp:` without host, `file:`): keep verbatim minus fragment.
        let mut url = url;
        url.set_fragment(None);
        return url.to_string();
    };
    let mut out = String::new();
    if !matches!(url.scheme(), "http" | "https") {
        out.push_str(url.scheme());
        out.push_str("://");
    }
    out.push_str(host);
    if let Some(port) = url.port() {
        out.push(':');
        out.push_str(&port.to_string());
    }
    match xtream_path_key(&url) {
        Some(key) => out.push_str(&key),
        None => out.push_str(url.path()),
    }
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| {
            !VOLATILE_QUERY_KEYS
                .iter()
                .any(|v| k.eq_ignore_ascii_case(v))
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if !pairs.is_empty() {
        pairs.sort();
        out.push('?');
        out.push_str(
            &url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(pairs)
                .finish(),
        );
    }
    out
}

/// `/live/U/P/123.ts` → `/xc/live/123`; `/U/P/123` → `/xc/live/123`;
/// `/movie/U/P/9.mkv` → `/xc/movie/9`.
fn xtream_path_key(url: &url::Url) -> Option<String> {
    let segments: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    let numeric_stem = |s: &str| {
        let stem = s.split_once('.').map_or(s, |(stem, _)| stem);
        (!stem.is_empty() && stem.bytes().all(|b| b.is_ascii_digit())).then(|| stem.to_owned())
    };
    match segments.as_slice() {
        [kind @ ("live" | "movie" | "series"), _, _, last] => {
            numeric_stem(last).map(|id| format!("/xc/{kind}/{}", canonical_provider_id(&id)))
        }
        [_, _, last] => {
            // The short live form has no extension or a stream one.
            let ext = last.split_once('.').map(|(_, e)| e);
            if matches!(ext, None | Some("ts" | "m3u8")) {
                numeric_stem(last).map(|id| format!("/xc/live/{}", canonical_provider_id(&id)))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Applies rule 3 (`~2`, `~3`, … for repeats) during one import.
#[derive(Default)]
pub(crate) struct IdDeduper {
    seen: HashMap<ContentId, u32>,
}

impl IdDeduper {
    pub(crate) fn unique(&mut self, id: ContentId) -> ContentId {
        let count = self.seen.entry(id.clone()).or_insert(0);
        *count += 1;
        if *count == 1 {
            id
        } else {
            ContentId(format!("{}~{}", id.0, count))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_ids_include_source_and_kind() {
        let live = ContentId::from_provider(SourceKind::Xtream, ContentKind::Live, "0042");
        let movie = ContentId::from_provider(SourceKind::Xtream, ContentKind::Movie, "42");
        assert_eq!(live.as_str(), "xt:live:42");
        assert_eq!(movie.as_str(), "xt:movie:42");
        assert_ne!(live, movie);
        assert_eq!(
            ContentId::from_provider(SourceKind::Stalker, ContentKind::Live, " ab-1 ").as_str(),
            "st:live:ab-1"
        );
    }

    #[test]
    fn m3u_id_survives_volatile_changes() {
        let base = ContentId::from_m3u(
            "http://cdn.example.com/hls/news.m3u8?token=aaa&q=1",
            "News One",
        );
        for variant in [
            "https://CDN.example.com:443/hls/news.m3u8?q=1&token=bbb",
            "http://cdn.example.com:80/hls/news.m3u8?q=1&expires=99#frag",
            "  http://user:pw@cdn.example.com/hls/news.m3u8?q=1|User-Agent=Test ",
        ] {
            assert_eq!(
                ContentId::from_m3u(variant, "  news   ONE "),
                base,
                "{variant}"
            );
        }
    }

    #[test]
    fn m3u_id_changes_with_identity() {
        let base = ContentId::from_m3u("http://cdn.example.com/hls/news.m3u8?q=1", "News One");
        assert_ne!(
            ContentId::from_m3u("http://cdn.example.com/hls/news.m3u8?q=2", "News One"),
            base
        );
        assert_ne!(
            ContentId::from_m3u("http://cdn.example.com/hls/other.m3u8?q=1", "News One"),
            base
        );
        assert_ne!(
            ContentId::from_m3u("http://cdn.example.com/hls/news.m3u8?q=1", "News Two"),
            base
        );
        assert_ne!(
            ContentId::from_m3u("http://cdn2.example.com/hls/news.m3u8?q=1", "News One"),
            base
        );
    }

    #[test]
    fn xtream_shaped_m3u_urls_ignore_credentials_and_extension() {
        let a = ContentId::from_m3u(
            "http://tv.example.net:8080/live/alice/old-pass/1001.ts",
            "Ch",
        );
        let b = ContentId::from_m3u(
            "https://tv.example.net:8080/live/alice/new-pass/1001.m3u8",
            "Ch",
        );
        let short = ContentId::from_m3u("http://tv.example.net:8080/alice/new-pass/1001", "Ch");
        assert_eq!(a, b);
        assert_eq!(a, short);
        let movie = ContentId::from_m3u("http://tv.example.net:8080/movie/alice/p/1001.mkv", "Ch");
        assert_ne!(a, movie);
    }

    #[test]
    fn id_format_is_pinned() {
        // Persisted keys: this value must never change without a migration.
        assert_eq!(
            normalize_stream_url("HTTP://Example.com:80/a/b.ts?z=1&a=2&token=x"),
            "example.com/a/b.ts?a=2&z=1"
        );
        let id = ContentId::from_m3u("http://example.com/a/b.ts", "Name");
        assert_eq!(
            id.as_str(),
            format!("m3u:{:016x}", xxh3_64(b"example.com/a/b.ts\nname"))
        );
    }

    #[test]
    fn deduper_suffixes_repeats_in_order() {
        let mut d = IdDeduper::default();
        let id = ContentId::from_m3u("http://example.com/x", "X");
        assert_eq!(d.unique(id.clone()), id);
        assert_eq!(d.unique(id.clone()).as_str(), format!("{}~2", id.as_str()));
        assert_eq!(d.unique(id.clone()).as_str(), format!("{}~3", id.as_str()));
    }

    #[test]
    fn group_ids() {
        assert_eq!(
            GroupId::from_provider(SourceKind::Xtream, ContentKind::Live, "7").as_str(),
            "xt:live-group:7"
        );
        assert_eq!(
            GroupId::from_m3u_title(" Sports "),
            GroupId::from_m3u_title("sports")
        );
    }
}

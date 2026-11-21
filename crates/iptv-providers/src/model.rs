//! Provider-neutral catalog model shared by M3U, Xtream and Stalker.
//!
//! Provider rows are replaced on every refresh; user customisations are keyed
//! by [`ContentId`]/[`GroupId`] (see [`crate::id`]) and survive that.

use std::fmt;

use crate::id::{ContentId, GroupId};
use crate::secret::{Secret, SecretUrl};

/// Which provider protocol a source speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SourceKind {
    M3u,
    Xtream,
    Stalker,
}

impl SourceKind {
    /// Prefix used in ids (`m3u`, `xt`, `st`). Part of the persisted id format.
    pub fn id_prefix(self) -> &'static str {
        match self {
            Self::M3u => "m3u",
            Self::Xtream => "xt",
            Self::Stalker => "st",
        }
    }
}

/// What a catalog row is. Providers reuse numeric ids across kinds, so the
/// kind is part of every id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ContentKind {
    Live,
    Radio,
    Movie,
    Series,
    Episode,
}

impl ContentKind {
    /// Slug used in ids. Part of the persisted id format.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Radio => "radio",
            Self::Movie => "movie",
            Self::Series => "series",
            Self::Episode => "episode",
        }
    }
}

/// A configured source: display name plus how to reach it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub name: String,
    pub config: SourceConfig,
}

impl Source {
    pub fn kind(&self) -> SourceKind {
        match self.config {
            SourceConfig::M3u(_) => SourceKind::M3u,
            SourceConfig::Xtream(_) => SourceKind::Xtream,
            SourceConfig::Stalker(_) => SourceKind::Stalker,
        }
    }
}

/// Connection details. Every secret is a [`Secret`]/[`SecretUrl`]/
/// [`MacAddress`], so `Debug` of a whole source is safe to log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceConfig {
    M3u(M3uSourceConfig),
    Xtream(XtreamCredentials),
    Stalker(StalkerCredentials),
}

#[derive(Clone, PartialEq, Eq)]
pub struct M3uSourceConfig {
    /// Playlist URL or local path; may embed credentials.
    pub location: SecretUrl,
    /// Playlist-wide User-Agent; a channel's `#EXTVLCOPT` value wins.
    pub user_agent: Option<String>,
    pub referrer: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct XtreamCredentials {
    /// Portal base as typed; normalise with [`crate::xtream::normalize_server_url`].
    pub server_url: String,
    pub username: Secret,
    pub password: Secret,
    pub user_agent: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct StalkerCredentials {
    /// Portal address as typed (`…/c/`, `…/stalker_portal/`, or an API `.php`).
    pub portal_url: String,
    pub mac: MacAddress,
    pub serial: Option<Secret>,
    pub device_id: Option<Secret>,
    pub device_id2: Option<Secret>,
    pub signature: Option<Secret>,
    pub signature2: Option<Secret>,
    /// For portals whose `get_profile` answers status 2 (login required).
    pub login: Option<Secret>,
    pub password: Option<Secret>,
}

/// A category (Xtream/Stalker) or group-title (M3U).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub id: GroupId,
    pub name: String,
    pub kind: ContentKind,
    /// The provider's own category id, when it has one.
    pub provider_id: Option<String>,
    /// Stalker `censored` genres and similar adult flags.
    pub adult: bool,
}

/// How to obtain a playable URL for an item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamLocator {
    /// A ready URL (M3U, Xtream `direct_source`, Stalker static `cmd`) plus
    /// per-item request headers and player properties (`#KODIPROP`).
    Url {
        url: String,
        headers: Vec<(String, String)>,
        properties: Vec<(String, String)>,
    },
    /// Build with [`crate::xtream::XtreamClient`] (live/movie/episode URL).
    Xtream {
        stream_id: u64,
        container_extension: Option<String>,
    },
    /// Resolve with the Stalker session: static `cmd` or `create_link`.
    Stalker { cmd: String, link: StalkerLinkFlags },
}

/// The two Stalker row flags that decide whether `create_link` is needed.
/// `None` means the key was absent, which is not the same as `false`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StalkerLinkFlags {
    pub use_http_tmp_link: Option<bool>,
    pub use_load_balancing: Option<bool>,
}

/// Catch-up (archive) capability of a live channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatchUp {
    pub kind: CatchUpKind,
    /// Archive window in days (> 0). `None` when the source declares
    /// catch-up without a window (an M3U `catchup` type but no
    /// `catchup-days`); the caller's default applies, see
    /// [`CatchUp::window_days`].
    pub days: Option<u32>,
    /// M3U `catchup-source` template, if declared.
    pub source: Option<String>,
    /// M3U `catchup-correction` (hours, may be fractional) in seconds, added
    /// to the programme times written into catch-up URLs. `0` when absent.
    pub correction_secs: i32,
}

impl CatchUp {
    /// The archive window to use: the declared one, else `default_days`.
    pub fn window_days(&self, default_days: u32) -> u32 {
        self.days.unwrap_or(default_days)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatchUpKind {
    /// `catchup="default"`: `source` is the complete replay URL template.
    Default,
    /// `catchup="append"`: `source` is appended to the live URL.
    Append,
    /// `catchup="shift"` (or archive days without a type): `utc`/`lutc`
    /// query parameters on the live URL.
    Shift,
    /// `catchup="flussonic"` / `fs` / `flussonic-hls` / `flussonic-ts`.
    Flussonic,
    /// `catchup="xc"` or an Xtream panel's `tv_archive`: timeshift URLs.
    Xtream,
    /// Stalker `tv_archive`; replay goes through the portal.
    Stalker,
    /// `catchup="vod"`: `source` (default `{catchup-id}`) is the complete
    /// URL of a recording, played as a video.
    Vod,
    /// A declared type this crate does not understand; replay unavailable.
    Unsupported(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Channel {
    pub id: ContentId,
    /// The provider's stream id (Xtream `stream_id`, Stalker `id`).
    pub provider_id: Option<String>,
    pub name: String,
    pub number: Option<u32>,
    pub logo: Option<String>,
    /// Usually one; M3U `group-title="A;B"` yields several.
    pub groups: Vec<GroupId>,
    pub stream: StreamLocator,
    /// XMLTV channel id (`tvg-id`, Xtream `epg_channel_id`, Stalker `xmltv_id`).
    pub epg_id: Option<String>,
    /// M3U `tvg-name`, used as a secondary EPG match key.
    pub epg_name: Option<String>,
    /// M3U `tvg-shift` (hours, may be fractional) converted to minutes.
    pub epg_shift_minutes: Option<i32>,
    pub catch_up: Option<CatchUp>,
    pub radio: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VodItem {
    pub id: ContentId,
    pub provider_id: Option<String>,
    pub name: String,
    pub groups: Vec<GroupId>,
    pub poster: Option<String>,
    pub plot: Option<String>,
    pub year: Option<u16>,
    /// Rating on a 0–10 scale, stored ×10 (`75` = 7.5) to stay `Eq`.
    pub rating_x10: Option<u16>,
    /// Unix seconds the provider added it.
    pub added: Option<i64>,
    pub duration_secs: Option<u32>,
    pub stream: StreamLocator,
    /// Stalker VOD rows flagged `is_series` open as series.
    pub is_series: bool,
    /// Embedded Stalker episode coordinates, used with the parent command.
    pub episode_numbers: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Series {
    pub id: ContentId,
    pub provider_id: Option<String>,
    pub name: String,
    pub groups: Vec<GroupId>,
    pub poster: Option<String>,
    pub plot: Option<String>,
    pub year: Option<u16>,
    pub rating_x10: Option<u16>,
    pub last_modified: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Season {
    pub number: u32,
    pub name: Option<String>,
    pub episode_count: Option<u32>,
    pub poster: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Episode {
    pub id: ContentId,
    pub provider_id: Option<String>,
    pub season: u32,
    pub number: u32,
    pub title: String,
    pub plot: Option<String>,
    pub image: Option<String>,
    pub duration_secs: Option<u32>,
    pub stream: StreamLocator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeriesDetails {
    pub series: Series,
    pub seasons: Vec<Season>,
    /// Sorted by (season, number). Malformed episodes are skipped.
    pub episodes: Import<Episode>,
}

/// One programme from a provider EPG call (Xtream `get_short_epg` /
/// `get_simple_data_table`). XMLTV is a separate crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    pub epg_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    /// Unix seconds.
    pub start: i64,
    pub end: i64,
    pub has_archive: bool,
}

/// Where a skipped or repaired entry was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WarningLocation {
    /// 1-based line in an M3U file.
    Line(u64),
    /// 0-based position in a JSON list.
    Index(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportWarning {
    pub location: WarningLocation,
    pub message: String,
}

/// Stored warnings are capped so a broken 100k-line file cannot balloon;
/// [`Import::warnings_dropped`] counts the rest.
pub const MAX_STORED_WARNINGS: usize = 1000;

/// Items parsed from one response or file. Bad entries are skipped with a
/// warning; they never abort the import.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import<T> {
    pub items: Vec<T>,
    /// Entries that were dropped.
    pub skipped: usize,
    pub warnings: Vec<ImportWarning>,
    pub warnings_dropped: usize,
}

impl<T> Default for Import<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            skipped: 0,
            warnings: Vec::new(),
            warnings_dropped: 0,
        }
    }
}

impl<T> Import<T> {
    pub(crate) fn warn(&mut self, location: WarningLocation, message: impl Into<String>) {
        if self.warnings.len() < MAX_STORED_WARNINGS {
            self.warnings.push(ImportWarning {
                location,
                message: message.into(),
            });
        } else {
            self.warnings_dropped += 1;
        }
    }

    pub(crate) fn skip(&mut self, location: WarningLocation, message: impl Into<String>) {
        self.skipped += 1;
        self.warn(location, message);
    }

    /// Total warnings including the ones not stored.
    pub fn warning_count(&self) -> usize {
        self.warnings.len() + self.warnings_dropped
    }
}

/// Infomir's OUI. Stock portals only accept MACs in it; reseller panels do
/// not check. Advisory only.
pub const INFOMIR_OUI: &str = "00:1A:79";

/// A Stalker MAC address. `Debug` shows only the OUI.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct MacAddress(String);

impl MacAddress {
    /// Normalises typed input (colons, hyphens, dots, spaces or none; any
    /// case) to `00:1A:79:12:34:56`. `None` if it is not 12 hex digits.
    pub fn parse(input: &str) -> Option<Self> {
        let digits: String = input
            .chars()
            .filter(|c| !matches!(c, ':' | '-' | '.') && !c.is_whitespace())
            .collect::<String>()
            .to_ascii_uppercase();
        if digits.len() != 12 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let pairs: Vec<&str> = (0..6).map(|i| &digits[i * 2..i * 2 + 2]).collect();
        Some(Self(pairs.join(":")))
    }

    /// A MAC read back from storage, used verbatim: rewriting the bytes a
    /// working portal accepted would change the account identity.
    pub fn from_stored(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value sent on the wire. Do not log.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn has_infomir_oui(&self) -> bool {
        Self::parse(&self.0).is_some_and(|m| m.0.starts_with(INFOMIR_OUI))
    }
}

impl fmt::Debug for MacAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match Self::parse(&self.0) {
            Some(m) => write!(f, "MacAddress({}:**:**:**)", &m.0[..8]),
            None => f.write_str("MacAddress(***)"),
        }
    }
}

impl std::fmt::Debug for M3uSourceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("M3uSourceConfig").finish_non_exhaustive()
    }
}

impl std::fmt::Debug for XtreamCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XtreamCredentials").finish_non_exhaustive()
    }
}

impl std::fmt::Debug for StalkerCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StalkerCredentials").finish_non_exhaustive()
    }
}

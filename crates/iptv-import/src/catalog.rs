//! The result of a source import.

use std::collections::HashSet;

use iptv_epg::FeedId;
use iptv_providers::m3u::M3uHeader;
use iptv_providers::model::{
    Channel, ContentKind, Group, Import, MAX_STORED_WARNINGS, Series, VodItem, WarningLocation,
};
use iptv_providers::stalker::StalkerSession;
use iptv_providers::xtream::AccountInfo;
use iptv_providers::{GroupId, SecretUrl};

use crate::epg::EpgSourceConfig;
use crate::error::Step;

/// What to import and the caller's clock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportOptions {
    #[cfg(debug_assertions)]
    pub allow_private_fixture: bool,
    pub limits: iptv_core::import_limits::ImportLimits,
    /// Unix seconds; used for Xtream expiry checks.
    pub now: i64,
    /// Xtream/Stalker movie categories and lists.
    pub include_vod: bool,
    /// Xtream and native Stalker series categories and lists.
    pub include_series: bool,
    /// Upper bound for a Stalker channel crawl.
    pub stalker_channel_cap: usize,
    pub stalker_vod_cap: usize,
    /// Extra attempts for a failed Stalker page before the crawl stops.
    pub page_retries: u8,
}

impl ImportOptions {
    /// Live channels only.
    pub fn new(now: i64) -> Self {
        Self {
            limits: Default::default(),
            #[cfg(debug_assertions)]
            allow_private_fixture: false,
            now,
            include_vod: false,
            include_series: false,
            stalker_channel_cap: 30_000,
            stalker_vod_cap: 100_000,
            page_retries: 1,
        }
    }

    pub fn with_vod(mut self, vod: bool, series: bool) -> Self {
        self.include_vod = vod;
        self.include_series = series;
        self
    }
}

/// Provider-specific facts the app keeps next to the catalog.
#[derive(Clone, Debug)]
pub enum SourceMeta {
    M3u {
        header: M3uHeader,
    },
    Xtream {
        /// Normalised portal base.
        base_url: String,
        /// `None` when no account probe answered with `user_info`.
        account: Option<AccountInfo>,
        /// The panel's XMLTV export (carries credentials).
        xmltv_url: SecretUrl,
    },
    Stalker {
        /// Authorised session: endpoint, mode, token and profile, needed for
        /// `create_link` and the watchdog. `Debug` is redacted.
        session: Box<StalkerSession>,
    },
}

/// A skipped or repaired entry, or an optional step that failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogWarning {
    pub step: Step,
    /// Stalker page the entry was on.
    pub page: Option<u32>,
    pub location: Option<WarningLocation>,
    pub message: String,
}

/// Counters for logs and the import summary.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportStats {
    /// Content records emitted, including those held only by a private sink.
    pub content_items: u64,
    pub requests: u32,
    /// Body bytes received (after transport decompression).
    pub bytes: u64,
    /// Entries dropped as malformed or duplicate.
    pub skipped: usize,
    /// Stalker pages fetched.
    pub pages: u32,
    /// Groups created for ids that items referenced but the category list
    /// lacked.
    pub synthesized_groups: usize,
}

/// A source's catalog in provider-neutral types.
#[derive(Clone, Debug)]
pub struct Catalog {
    pub source: SourceMeta,
    /// Live groups first, then VOD and series groups, in provider order.
    pub groups: Vec<Group>,
    pub channels: Vec<Channel>,
    pub vod: Vec<VodItem>,
    pub series: Vec<Series>,
    pub warnings: Vec<CatalogWarning>,
    /// Warnings beyond the stored cap.
    pub warnings_dropped: usize,
    /// Movie or series lists that could not be downloaded while the rest
    /// of the import went on, with why (first failure of each kind).
    pub on_demand_failures: Vec<(ContentKind, crate::error::ImportError)>,
    pub stats: ImportStats,
}

/// Discard surplus declared guides before fetching or validating their destinations.
pub(crate) fn limit_header_guides(header: &mut M3uHeader) -> usize {
    let dropped = header
        .epg_urls
        .len()
        .saturating_sub(iptv_library::guides::MAX_PROVIDER_GUIDES);
    header
        .epg_urls
        .truncate(iptv_library::guides::MAX_PROVIDER_GUIDES);
    dropped
}

impl Catalog {
    pub(crate) fn new(mut source: SourceMeta) -> Self {
        let dropped = match &mut source {
            SourceMeta::M3u { header } => limit_header_guides(header),
            _ => 0,
        };
        let mut catalog = Self {
            source,
            groups: Vec::new(),
            channels: Vec::new(),
            vod: Vec::new(),
            series: Vec::new(),
            warnings: Vec::new(),
            warnings_dropped: 0,
            on_demand_failures: Vec::new(),
            stats: ImportStats::default(),
        };
        catalog.warn_guide_limit(dropped);
        catalog
    }

    pub(crate) fn warn_guide_limit(&mut self, dropped: usize) {
        if dropped > 0 {
            self.warn(
                Step::Playlist,
                format!(
                    "ignored {dropped} surplus provider guide URLs (limit {})",
                    iptv_library::guides::MAX_PROVIDER_GUIDES
                ),
            );
        }
    }

    /// No channels, movies or series.
    pub fn is_empty(&self) -> bool {
        self.stats.content_items == 0
            && self.channels.is_empty()
            && self.vod.is_empty()
            && self.series.is_empty()
    }

    /// Total warnings including the ones not stored.
    pub fn warning_count(&self) -> usize {
        self.warnings.len() + self.warnings_dropped
    }

    /// EPG sources the source itself declares: the M3U header's
    /// `url-tvg`/`x-tvg-url`, or the Xtream panel's `xmltv.php`. Ids start
    /// at `first_id`; earlier URLs get higher priority.
    pub fn epg_sources(
        &self,
        first_id: u32,
    ) -> Result<Vec<EpgSourceConfig>, iptv_library::guides::GuideFeedError> {
        match &self.source {
            SourceMeta::M3u { header } => EpgSourceConfig::from_m3u_header(header, first_id),
            SourceMeta::Xtream { xmltv_url, .. } => Ok(vec![EpgSourceConfig::new(
                FeedId(first_id),
                xmltv_url.clone(),
            )]),
            SourceMeta::Stalker { .. } => Ok(Vec::new()),
        }
    }

    /// The provider's own guide addresses, in its order (the playlist
    /// header's, or the Xtream panel's `xmltv.php`; none for Stalker).
    pub fn guide_urls(&self) -> Vec<SecretUrl> {
        match &self.source {
            SourceMeta::M3u { header } => header
                .epg_urls
                .iter()
                .map(|url| SecretUrl::new(url.clone()))
                .collect(),
            SourceMeta::Xtream { xmltv_url, .. } => vec![xmltv_url.clone()],
            SourceMeta::Stalker { .. } => Vec::new(),
        }
    }

    /// Copies the request counters gathered by the driver.
    pub(crate) fn set_transport_stats(&mut self, s: &ImportStats) {
        self.stats.requests = s.requests;
        self.stats.bytes = s.bytes;
        self.stats.pages = s.pages;
    }

    pub(crate) fn warn(&mut self, step: Step, message: impl Into<String>) {
        self.push_warning(CatalogWarning {
            step,
            page: None,
            location: None,
            message: message.into(),
        });
    }

    pub(crate) fn push_warning(&mut self, w: CatalogWarning) {
        if self.warnings.len() < MAX_STORED_WARNINGS {
            self.warnings.push(w);
        } else {
            self.warnings_dropped += 1;
        }
    }

    /// Moves an [`Import`]'s warnings and skip count into the catalog and
    /// returns its items.
    pub(crate) fn absorb<T>(&mut self, step: Step, page: Option<u32>, import: Import<T>) -> Vec<T> {
        self.stats.skipped += import.skipped;
        self.warnings_dropped += import.warnings_dropped;
        for w in import.warnings {
            self.push_warning(CatalogWarning {
                step,
                page,
                location: Some(w.location),
                message: w.message,
            });
        }
        import.items
    }

    /// Adds a placeholder group for every id that `items` reference but no
    /// category declared, so nothing is orphaned.
    pub(crate) fn add_missing_groups<'a>(
        &mut self,
        kind: ContentKind,
        referenced: impl Iterator<Item = &'a GroupId>,
    ) {
        let mut known: HashSet<GroupId> = self.groups.iter().map(|g| g.id.clone()).collect();
        let mut added = Vec::new();
        for id in referenced {
            if known.insert(id.clone()) {
                let provider_id = id.as_str().split_once("-group:").map(|(_, p)| p.to_owned());
                added.push(Group {
                    name: match &provider_id {
                        Some(p) => format!("Category {p}"),
                        None => "Other".to_owned(),
                    },
                    id: id.clone(),
                    kind,
                    provider_id,
                    adult: false,
                });
            }
        }
        self.stats.synthesized_groups += added.len();
        self.groups.extend(added);
    }
}

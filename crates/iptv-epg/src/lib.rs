#![forbid(unsafe_code)]
//! EPG layer of iptv-rs.
//!
//! - [`XmltvParser`]: sans-IO, incremental XMLTV parser. Feed it bytes in
//!   any chunking (plain or gzip); it yields [`XmltvChannel`] and
//!   [`XmltvProgram`] records, skipping malformed entries with counted
//!   [`ParseWarnings`].
//! - [`MatchIndex`]: maps playlist channels ([`ChannelKey`]) to EPG channel
//!   ids by tvg-id, then by normalized name, with manual [`Override`]s.
//! - [`MatchFilter`]: before a feed is matched, which of its EPG channels
//!   some playlist channel could match (a streaming import keeps only
//!   their programs).
//! - [`SourceBuilder`] / [`GuideStore`]: compact per-channel schedules from
//!   several sources with priorities, retention and O(log n) guide queries.
//!   [`ScheduleRules`] are their ordering, overlap and retention rules, for
//!   stores that settle a schedule outside memory; [`SettledBuilder`]
//!   reads such a settled schedule back without sorting it again.
//!
//! No NSH, no platform APIs, no clock, no blocking I/O; builds for
//! `wasm32-unknown-unknown`. Times are unix seconds ([`UnixTime`]) and every
//! time-dependent call takes `now` from the caller.

mod decode;
mod extra;
mod intern;
mod matching;
mod normalize;
mod parser;
mod settle;
mod settled_source;
mod store;
mod strict;
mod superscript;
pub use strict::{StrictError, StrictXmltvParser};
mod time;
mod xmltv;

pub use decode::{Compression, DecodeError};
pub use matching::{
    ChannelKey, EpgRef, FeedId, MatchFilter, MatchIndex, MatchKind, MatchReport, MatchResult,
    Override, Overrides, PlaylistChannel,
};
pub use normalize::{normalize_name, strip_tag_prefix};
pub use parser::{ParseOptions, ParseStats, ParseWarnings, ParsedXmltv, XmltvParser, parse_xmltv};
pub use settle::{ScheduleRules, Settled};
pub use settled_source::SettledBuilder;
pub use store::{
    EpgSource, GuideStore, IngestReport, NowNext, ProgramData, ProgramRef, ScheduleRef,
    SourceBuilder, StoreOptions,
};
pub use superscript::fold_superscripts;
pub use time::{DAY, TimePolicy, UnixTime, format_xmltv_time, parse_xmltv_time};
pub use xmltv::{EpisodeNum, Rating, XmltvChannel, XmltvItem, XmltvProgram};

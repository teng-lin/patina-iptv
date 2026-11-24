//! Search: channels by name and programs by title, now on or coming up,
//! with `iptv-search`'s indexes and its search screen state
//! ([`SearchState`]). Everything here runs in the worker, so a search
//! never holds the page's remote handling.
//!
//! The channel index is made when search first opens (a scan of the
//! channel names). The program index covers every stored program and is
//! built in steps of [`BUILD_CHANNELS`] channels, yielding to the worker's
//! event loop between them, so keys keep their pace while it builds; until
//! it is ready only channels are found. Both are dropped when the guide
//! changes and made again at the next search.

use crate::presentation::Presentation;

use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::Duration;

use iptv_core::favorites::KeyIndex;
use iptv_core::guide::{Airing, Schedule};
use iptv_guide::EpgSchedule;
use iptv_presentation::search::{Focus, Phase, ResultId, SearchState, Tab};
use iptv_search::{
    ChannelHit, ChannelId, ChannelIndex, ChannelSearchOptions, ProgramHit, ProgramIndex,
    ProgramIndexBuilder, ProgramSearchOptions,
};
use serde_json::{Value, json};

/// Result rows the search screen shows at once.
pub(crate) const SEARCH_ROWS: usize = 8;
/// Results kept per tab.
const LIMIT: usize = 100;
/// Channels indexed per step of the program index build.
pub(crate) const BUILD_CHANNELS: usize = 250;
/// How far ahead programs are found (current and upcoming ones only).
const LOOK_AHEAD: i64 = 3 * iptv_epg::DAY;

/// The indexes of the current guide.
#[derive(Default)]
pub(crate) struct Indexes {
    channels: Option<Rc<ChannelIndex>>,
    programs: Option<Rc<ProgramIndex>>,
    building: bool,
    /// Bumped when the guide changes: a build for an older one stops.
    generation: u32,
}

/// What building the program index took, for the host's measurements.
pub(crate) struct Built {
    pub(crate) ms: f64,
    pub(crate) programs: usize,
    pub(crate) bytes: usize,
}

impl Indexes {
    /// The guide changed: both indexes are made again when next needed.
    pub(crate) fn reset(&mut self) {
        self.channels = None;
        self.programs = None;
        self.building = false;
        self.generation = self.generation.wrapping_add(1);
    }

    pub(crate) fn channels(&mut self, s: &dyn Schedule) -> Rc<ChannelIndex> {
        self.channels
            .get_or_insert_with(|| Rc::new(ChannelIndex::from_schedule(s)))
            .clone()
    }

    pub(crate) fn programs(&self) -> Option<&ProgramIndex> {
        self.programs.as_deref()
    }

    /// Whether program results are still being prepared.
    pub(crate) fn preparing(&self) -> bool {
        self.building
    }

    /// Starts building the program index unless it exists or is being
    /// built; returns the build's generation to pass to [`build_programs`].
    pub(crate) fn start_build(&mut self) -> Option<u32> {
        if self.programs.is_some() || self.building {
            return None;
        }
        self.building = true;
        Some(self.generation)
    }

    /// Installs a finished index, unless the guide changed meanwhile.
    pub(crate) fn finish_build(&mut self, generation: u32, index: ProgramIndex) -> bool {
        if generation != self.generation {
            return false;
        }
        self.programs = Some(Rc::new(index));
        self.building = false;
        true
    }

    pub(crate) fn current(&self, generation: u32) -> bool {
        self.generation == generation && self.building
    }
}

/// Builds the program index of `schedule` in steps; `current` says the
/// build is still wanted. Returns the index and what it took, or `None`
/// when it was abandoned.
pub(crate) async fn build_programs(
    schedule: Rc<EpgSchedule>,
    current: impl Fn() -> bool,
) -> Option<(ProgramIndex, Built)> {
    let started = crate::host::host().monotonic_ms();
    let n = schedule.channel_count();
    let mut builder = ProgramIndexBuilder::new(n);
    let store = schedule.store();
    for from in (0..n).step_by(BUILD_CHANNELS) {
        for c in from..(from + BUILD_CHANNELS).min(n) {
            let Some(channel) = schedule.channel(c) else {
                continue;
            };
            builder.set_identity(c, ChannelId::new(None, channel.key.as_str()));
            if schedule.channel_hidden(c) {
                continue;
            }
            if let Some(programs) = store.schedule(&channel.key) {
                for p in programs.iter() {
                    builder.push(c, p.start(), p.stop(), p.title());
                }
            }
        }
        // Lets the worker take the keys that arrived meanwhile.
        crate::exec::sleep_while(Duration::ZERO, &current).await;
        if !current() {
            return None;
        }
    }
    let programs = builder.len();
    let index = builder.finish();
    let built = Built {
        ms: crate::host::host().monotonic_ms() - started,
        programs,
        bytes: index.heap_bytes(),
    };
    Some((index, built))
}

/// Both searches for `query` at `now` (Unix seconds): channels by name,
/// and programs airing now or within [`LOOK_AHEAD`] when the program
/// index is ready. Favorites rank first.
pub(crate) fn run(
    channels: &ChannelIndex,
    programs: Option<&ProgramIndex>,
    query: &str,
    favorites: &BTreeSet<usize>,
    now: i64,
) -> (Vec<ChannelHit>, Vec<ProgramHit>) {
    let channel_hits = channels.search_channels(
        query,
        favorites,
        ChannelSearchOptions {
            favorites_first: true,
            limit: LIMIT,
        },
    );
    let program_hits = programs.map_or_else(Vec::new, |index| {
        index.search_programs(
            query,
            now,
            ProgramSearchOptions {
                look_back: 0,
                look_ahead: LOOK_AHEAD,
                favorites: Some(favorites),
                favorites_first: true,
                limit: LIMIT,
                ..ProgramSearchOptions::default()
            },
        )
    });
    (channel_hits, program_hits)
}

/// The program `hits` found in a guide since replaced that `schedule` (the
/// new guide of the same catalog) still lists: the same title at the same
/// start on that channel. Each keeps the new listing's end and where it
/// airs at `now` (Unix seconds); one the new guide ended by then is gone,
/// as a search at `now` finds none that ended.
pub(crate) fn still_listed(
    schedule: &EpgSchedule,
    hits: &[ProgramHit],
    now: i64,
) -> Vec<ProgramHit> {
    let store = schedule.store();
    hits.iter()
        .filter_map(|h| {
            let p = schedule
                .channel(h.channel)
                .filter(|_| !schedule.channel_hidden(h.channel))
                .and_then(|c| store.schedule(&c.key))
                .and_then(|programs| programs.at(h.start))
                .filter(|p| p.start() == h.start && p.title() == h.title && p.stop() > now)?;
            let airing = if p.start() <= now {
                Airing::Live
            } else {
                Airing::Upcoming
            };
            Some(ProgramHit {
                end: p.stop(),
                airing,
                ..h.clone()
            })
        })
        .collect()
}

/// The catalog channel a result plays.
pub(crate) fn channel_of(id: &ResultId, keys: &KeyIndex) -> Option<usize> {
    keys.get(&id.channel().content)
}

fn window(focus: usize, len: usize) -> usize {
    focus
        .saturating_sub(SEARCH_ROWS / 2)
        .min(len.saturating_sub(SEARCH_ROWS))
}

/// One row of the screen by its index in the full list.
type Row<'a> = Box<dyn Fn(usize) -> Value + 'a>;

fn tab_name(tab: Tab) -> &'static str {
    match tab {
        Tab::Channels => "channels",
        Tab::Programs => "programs",
    }
}

/// The search screen: the query, where the focus is, and the rows on
/// screen (the history before a query, else the shown tab's results).
pub(crate) fn json(
    state: &SearchState,
    s: &dyn Schedule,
    preparing: bool,
    logo: &dyn Fn(usize) -> Option<String>,
    presentation: &Presentation,
) -> Value {
    let focus = match state.focus() {
        Focus::Input => "input",
        Focus::History(_) => "history",
        Focus::Tabs => "tabs",
        // Not offered here: the TV host searches no finished programs.
        Focus::Filter => "filter",
        Focus::Result(_) => "result",
    };
    let phase = match state.phase() {
        Phase::Idle => "idle",
        Phase::Typing => "typing",
        Phase::Results => "results",
        Phase::NoResults => "none",
    };
    let channel = |c: usize| {
        json!({
            "number": s.channel_number(c),
            "name": s.channel_name(c),
            "logo": logo(c),
        })
    };
    let (total, focused, items): (usize, Option<usize>, Row<'_>) = if state.phase() == Phase::Idle {
        let history = state.history().entries();
        let focused = match state.focus() {
            Focus::History(e) => history.iter().position(|h| h == e),
            _ => None,
        };
        (
            history.len(),
            focused,
            Box::new(move |i| json!({ "kind": "history", "text": history[i] })),
        )
    } else {
        match state.tab() {
            Tab::Channels => {
                let hits = state.channels();
                let focused = match state.focus() {
                    Focus::Result(ResultId::Channel(id)) => {
                        hits.iter().position(|h| &h.identity == id)
                    }
                    _ => None,
                };
                (
                    hits.len(),
                    focused,
                    Box::new(move |i| {
                        let mut v = channel(hits[i].channel);
                        v["kind"] = "channel".into();
                        v
                    }),
                )
            }
            Tab::Programs => {
                let hits = state.programs();
                let focused = match state.focus() {
                    Focus::Result(ResultId::Program { channel, start }) => hits
                        .iter()
                        .position(|h| &h.identity == channel && h.start == *start),
                    _ => None,
                };
                (
                    hits.len(),
                    focused,
                    Box::new(move |i| {
                        let h = &hits[i];
                        let mut v = channel(h.channel);
                        v["kind"] = "program".into();
                        v["title"] = h.title.clone().into();
                        v["start"] = h.start.into();
                        v["end"] = h.end.into();
                        v["timeLabel"] = presentation.span(h.start, h.end, "–").into();
                        v["live"] = (h.airing == Airing::Live).into();
                        v
                    }),
                )
            }
        }
    };
    let start = window(focused.unwrap_or(0), total);
    let rows: Vec<Value> = (start..(start + SEARCH_ROWS).min(total))
        .map(items)
        .collect();
    json!({
        "query": state.query(),
        "keyboard": state.keyboard_visible(),
        "focus": focus,
        "phase": phase,
        "tab": tab_name(state.tab()),
        "counts": { "channels": state.count(Tab::Channels), "programs": state.count(Tab::Programs) },
        "preparing": preparing,
        "rows": { "start": start, "focus": focused, "total": total, "items": rows },
    })
}

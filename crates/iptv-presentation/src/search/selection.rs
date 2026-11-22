//! What selecting a search result asks the presenter to show.

use iptv_core::guide::Airing;
use iptv_epg::UnixTime;

use super::ResultId;
use iptv_search::ProgramHit;

/// What a presenter supports when a search result is selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultPolicy {
    /// Plays live and archived programs, or shows program details.
    Guide,
    /// Opens the result's channel live (the bridge has no descriptions).
    LiveChannel,
}

/// What choosing a result does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultSelection {
    /// Channels and programs on now: the channel, live.
    Live(usize),
    /// A past program still in the archive: replay from its start.
    CatchUp {
        channel: usize,
        start: UnixTime,
    },
    /// Upcoming programs (and past ones without replay): their details.
    Details {
        channel: usize,
        start: UnixTime,
    },
    Nothing,
}

/// Selects what a resolved result does. The guide policy uses the matching
/// program hit, which may have disappeared since the result was selected.
pub fn select_result(
    id: &ResultId,
    hit: Option<&ProgramHit>,
    channel: Option<usize>,
    policy: ResultPolicy,
) -> ResultSelection {
    let Some(channel) = channel else {
        return ResultSelection::Nothing;
    };
    if policy == ResultPolicy::LiveChannel {
        return ResultSelection::Live(channel);
    }
    match (id, hit) {
        (ResultId::Channel(_), _) => ResultSelection::Live(channel),
        (ResultId::Program { .. }, None) => ResultSelection::Nothing,
        (ResultId::Program { start, .. }, Some(h)) => match h.airing {
            Airing::Live => ResultSelection::Live(channel),
            Airing::Ended if h.catchup => ResultSelection::CatchUp {
                channel,
                start: *start,
            },
            Airing::Ended | Airing::Upcoming => ResultSelection::Details {
                channel,
                start: *start,
            },
        },
    }
}

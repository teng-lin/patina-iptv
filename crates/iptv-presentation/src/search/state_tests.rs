use iptv_core::guide::Airing;

use super::SearchAction::*;
use super::*;
use iptv_search::MatchQuality;

fn ch(channel: usize) -> ChannelHit {
    ChannelHit {
        identity: iptv_search::ChannelId::new(None, format!("synthetic-{channel}")),
        channel,
        score: MatchQuality::Prefix,
    }
}

fn pr(channel: usize, start: UnixTime) -> ProgramHit {
    ProgramHit {
        identity: iptv_search::ChannelId::new(None, format!("synthetic-{channel}")),
        channel,
        start,
        end: start + 1800,
        title: format!("Show {channel}-{start}"),
        airing: Airing::Upcoming,
        catchup: false,
        score: MatchQuality::Word,
    }
}

fn typed(q: &str) -> SearchState {
    let mut s = SearchState::new(SearchHistory::default());
    assert_eq!(s.apply(Type(q.into())), SearchEffect::Search(q.into()));
    s
}

const P: fn(usize, UnixTime) -> ResultId = |channel, start| ResultId::Program {
    channel: iptv_search::ChannelId::new(None, format!("synthetic-{channel}")),
    start,
};

#[test]
fn idle_shows_history_and_picks_an_entry() {
    let history = SearchHistory::from_list(["kestrel", "orchard"], 20);
    let mut s = SearchState::new(history);
    assert_eq!(s.phase(), Phase::Idle);
    assert!(s.keyboard_visible());
    s.apply(Down);
    s.apply(Down);
    assert_eq!(s.focus(), &Focus::History("orchard".into()));
    s.apply(Down);
    assert_eq!(
        s.focus(),
        &Focus::History("orchard".into()),
        "stays on the last"
    );
    assert_eq!(s.apply(Ok), SearchEffect::Search("orchard".into()));
    assert_eq!((s.query(), s.phase()), ("orchard", Phase::Typing));
    assert!(!s.keyboard_visible());
    assert_eq!(s.history().entries(), ["orchard", "kestrel"]);
    // Keyboard hidden: results take focus.
    assert!(s.set_results("orchard", vec![ch(5)], vec![]));
    assert_eq!(
        s.focus(),
        &Focus::Result(ResultId::Channel(iptv_search::ChannelId::new(
            None,
            "synthetic-5"
        )))
    );
    // Up from history's first entry goes back to the input.
    let mut s = SearchState::new(SearchHistory::from_list(["one"], 20));
    s.apply(Down);
    s.apply(Up);
    assert_eq!(s.focus(), &Focus::Input);
}

#[test]
fn typing_drops_stale_results_and_submit_moves_focus() {
    let mut s = typed("ke");
    assert_eq!(
        s.apply(Type("kes".into())),
        SearchEffect::Search("kes".into())
    );
    assert!(!s.set_results("ke", vec![ch(1)], vec![]), "stale");
    assert_eq!(s.phase(), Phase::Typing);
    assert!(s.set_results("kes", vec![ch(1), ch(7)], vec![pr(2, 100)]));
    assert_eq!(s.phase(), Phase::Results);
    assert_eq!(s.focus(), &Focus::Input, "keyboard still up");
    assert_eq!(s.apply(Ok), SearchEffect::None);
    assert!(!s.keyboard_visible());
    assert_eq!(
        s.focus(),
        &Focus::Result(ResultId::Channel(iptv_search::ChannelId::new(
            None,
            "synthetic-1"
        )))
    );
    assert_eq!(s.history().entries(), ["kes"]);
}

#[test]
fn moves_between_input_tabs_and_results_keeping_focus_per_tab() {
    let mut s = typed("owl");
    s.set_results("owl", vec![ch(1), ch(7)], vec![pr(2, 100), pr(3, 50)]);
    s.apply(Down);
    assert_eq!(s.focus(), &Focus::Tabs);
    s.apply(Down);
    s.apply(Down);
    s.apply(Down);
    assert_eq!(
        s.focus(),
        &Focus::Result(ResultId::Channel(iptv_search::ChannelId::new(
            None,
            "synthetic-7"
        )))
    );
    s.apply(Right);
    assert_eq!(s.tab(), Tab::Programs);
    assert_eq!(s.focus(), &Focus::Result(P(2, 100)));
    s.apply(Down);
    s.apply(Left);
    assert_eq!(
        s.focus(),
        &Focus::Result(ResultId::Channel(iptv_search::ChannelId::new(
            None,
            "synthetic-7"
        ))),
        "remembered"
    );
    s.apply(Right);
    assert_eq!(s.focus(), &Focus::Result(P(3, 50)), "remembered");
    s.apply(Up);
    s.apply(Up);
    assert_eq!(s.focus(), &Focus::Tabs);
    s.apply(Left);
    assert_eq!((s.tab(), s.focus()), (Tab::Channels, &Focus::Tabs));
    s.apply(Up);
    assert_eq!(s.focus(), &Focus::Input);
}

#[test]
fn play_and_come_back_keeps_query_and_focused_result() {
    let mut s = typed("owl");
    s.set_results("owl", vec![ch(1)], vec![pr(2, 100), pr(3, 50), pr(3, 900)]);
    s.apply(Ok);
    s.apply(Right);
    s.apply(Down);
    assert_eq!(s.focus(), &Focus::Result(P(3, 50)));
    assert_eq!(s.apply(Ok), SearchEffect::Play(P(3, 50)));
    // Back from the player: refresh, results come back in another order.
    assert_eq!(s.resume(), SearchEffect::Search("owl".into()));
    s.set_results("owl", vec![ch(1)], vec![pr(3, 50), pr(9, 10), pr(2, 100)]);
    assert_eq!((s.query(), s.tab()), ("owl", Tab::Programs));
    assert_eq!(
        s.focus(),
        &Focus::Result(P(3, 50)),
        "same item, not same row"
    );
    // The program ended and dropped out: the same channel's next one.
    s.resume();
    s.set_results("owl", vec![ch(1)], vec![pr(2, 100), pr(3, 900)]);
    assert_eq!(s.focus(), &Focus::Result(P(3, 900)));
    // Nothing left on that channel: the first result.
    s.resume();
    s.set_results("owl", vec![ch(1)], vec![pr(4, 1), pr(2, 100)]);
    assert_eq!(s.focus(), &Focus::Result(P(4, 1)));
    assert_eq!(s.history().entries(), ["owl"]);
}

#[test]
fn switches_to_the_tab_with_results_and_handles_no_results() {
    let mut s = typed("nuit");
    s.set_results("nuit", vec![], vec![pr(1, 0)]);
    assert_eq!(s.tab(), Tab::Programs);
    s.apply(Ok);
    assert_eq!(s.focus(), &Focus::Result(P(1, 0)));
    s.apply(Left);
    assert_eq!(s.tab(), Tab::Programs, "no empty tab to switch to");
    s.apply(Type("nuitx".into()));
    s.set_results("nuitx", vec![], vec![]);
    assert_eq!((s.phase(), s.focus()), (Phase::NoResults, &Focus::Input));
    s.apply(Down);
    assert_eq!(s.focus(), &Focus::Input);
}

#[test]
fn back_hides_keyboard_then_returns_to_input_then_closes() {
    let mut s = typed("owl");
    s.set_results("owl", vec![ch(1)], vec![]);
    assert_eq!(s.apply(Back), SearchEffect::None);
    assert!(!s.keyboard_visible());
    assert_eq!(s.phase(), Phase::Results, "results stay");
    s.apply(Down);
    s.apply(Down);
    assert_eq!(s.apply(Back), SearchEffect::None);
    assert_eq!(s.focus(), &Focus::Input);
    assert_eq!(s.apply(Back), SearchEffect::Close);
    s.apply(ShowKeyboard);
    assert!(s.keyboard_visible());
    assert_eq!(s.apply(Clear), SearchEffect::None);
    assert_eq!((s.phase(), s.query()), (Phase::Idle, ""));
    assert!(s.channels().is_empty());
    assert!(
        !s.set_results("", vec![ch(1)], vec![]),
        "idle ignores results"
    );
    assert_eq!(s.apply(Back), SearchEffect::Close);
    assert_eq!(s.resume(), SearchEffect::None);
}

#[test]
fn taps_pick_history_results_and_tabs() {
    let mut s = SearchState::new(SearchHistory::from_list(["kestrel", "orchard"], 20));
    assert_eq!(
        s.apply(Pick(Focus::History("gone".into()))),
        SearchEffect::None
    );
    assert_eq!(s.apply(Pick(Focus::Tabs)), SearchEffect::None);
    assert_eq!(
        s.apply(Pick(Focus::History("orchard".into()))),
        SearchEffect::Search("orchard".into())
    );
    assert!(!s.keyboard_visible());
    assert!(s.set_results("orchard", vec![ch(1), ch(2)], vec![pr(3, 100), pr(4, 200)]));
    // A program tap switches tab, remembers the result and plays it.
    assert_eq!(
        s.apply(Pick(Focus::Result(P(4, 200)))),
        SearchEffect::Play(P(4, 200))
    );
    assert_eq!(
        (s.tab(), s.focus()),
        (Tab::Programs, &Focus::Result(P(4, 200)))
    );
    assert_eq!(
        s.apply(Pick(Focus::Result(P(9, 9)))),
        SearchEffect::None,
        "not a result"
    );
    // Tab labels: back to channels lands on the first, then the remembered one.
    s.apply(ShowTab(Tab::Channels));
    assert_eq!(
        s.focus(),
        &Focus::Result(ResultId::Channel(iptv_search::ChannelId::new(
            None,
            "synthetic-1"
        )))
    );
    s.apply(ShowTab(Tab::Programs));
    assert_eq!(s.focus(), &Focus::Result(P(4, 200)));
    // A tab without results does not open; from the field, focus goes to the tabs.
    assert!(s.set_results("orchard", vec![ch(1)], vec![]));
    s.apply(Pick(Focus::Input));
    assert!(s.keyboard_visible());
    s.apply(ShowTab(Tab::Programs));
    assert_eq!(s.tab(), Tab::Channels);
    s.apply(ShowTab(Tab::Channels));
    assert_eq!(s.focus(), &Focus::Tabs);
}

#[test]
fn keyboard_hidden_by_the_platform_keeps_the_screen() {
    let mut s = typed("kes");
    s.apply(KeyboardHidden);
    assert!(!s.keyboard_visible());
    assert_eq!((s.query(), s.focus()), ("kes", &Focus::Input));
    // The next Back leaves (nothing else to close).
    assert_eq!(s.apply(Back), SearchEffect::Close);
    // Idle: tapping history entries works, results do not exist.
    let mut s = SearchState::new(SearchHistory::default());
    assert_eq!(
        s.apply(Pick(Focus::Result(ResultId::Channel(
            iptv_search::ChannelId::new(None, "synthetic-0")
        )))),
        SearchEffect::None
    );
}

#[test]
fn catalog_reordering_preserves_semantic_focus_and_play_identity() {
    let mut state = typed("synthetic");
    let first = ch(3);
    let id = ResultId::Channel(first.identity.clone());
    state.set_results("synthetic", vec![first.clone()], Vec::new());
    state.apply(Pick(Focus::Result(id.clone())));
    let mut reordered = first;
    reordered.channel = 0;
    state.set_results("synthetic", vec![reordered], Vec::new());
    assert_eq!(state.focus(), &Focus::Result(id.clone()));
    assert_eq!(state.apply(Ok), SearchEffect::Play(id));
}

#[test]
fn program_fallback_stays_on_the_semantic_channel_after_positions_change() {
    let mut state = typed("synthetic");
    let original = pr(3, 100);
    state.set_results("synthetic", Vec::new(), vec![original.clone()]);
    state.apply(Pick(Focus::Result(P(3, 100))));
    let mut replacement = original;
    replacement.channel = 9;
    replacement.start = 200;
    state.set_results("synthetic", Vec::new(), vec![replacement]);
    assert_eq!(state.focus(), &Focus::Result(P(3, 200)));
}

#[test]
fn program_fallback_never_follows_a_recreated_source_with_the_same_content_key() {
    let mut state = typed("synthetic");
    let old_channel =
        iptv_search::ChannelId::in_source(1, "generated-original", "shared-provider-key");
    let mut original = pr(3, 100);
    original.identity = old_channel.clone();
    state.set_results("synthetic", Vec::new(), vec![original.clone()]);
    state.apply(Pick(Focus::Result(ResultId::Program {
        channel: old_channel.clone(),
        start: 100,
    })));
    let mut refreshed = original.clone();
    refreshed.channel = 9;
    refreshed.start = 200;
    state.set_results("synthetic", Vec::new(), vec![pr(0, 200), refreshed]);
    assert_eq!(
        state.focus(),
        &Focus::Result(ResultId::Program {
            channel: old_channel,
            start: 200
        })
    );
    let first = pr(0, 300);
    let first_id = ResultId::Program {
        channel: first.identity.clone(),
        start: first.start,
    };
    let mut recreated = original;
    recreated.identity =
        iptv_search::ChannelId::in_source(1, "generated-recreated", "shared-provider-key");
    recreated.channel = 9;
    recreated.start = 300;
    state.set_results("synthetic", Vec::new(), vec![first, recreated]);
    assert_eq!(state.focus(), &Focus::Result(first_id.clone()));
    assert_eq!(state.apply(Ok), SearchEffect::Play(first_id));
}

#[test]
fn duplicate_name_only_rows_have_distinct_index_scoped_navigation_identity() {
    let mut index = iptv_search::ChannelIndex::from_names(["Generated", "Generated"]);
    index.add_key(0, "first-extra");
    index.add_key(1, "second-extra");
    let hits = index.search_channels("generated", &Default::default(), Default::default());
    assert_eq!(hits.len(), 2);
    assert_ne!(hits[0].identity, hits[1].identity);
    assert!(hits.iter().all(|hit| hit.identity.is_anonymous()));
    let mut state = typed("generated");
    state.set_results("generated", hits.clone(), Vec::new());
    state.apply(Down);
    state.apply(Down);
    assert_eq!(
        state.focus(),
        &Focus::Result(ResultId::Channel(hits[0].identity.clone()))
    );
    state.apply(Down);
    assert_eq!(
        state.apply(Ok),
        SearchEffect::Play(ResultId::Channel(hits[1].identity.clone()))
    );
    assert_eq!(
        index.search_channels("second-extra", &Default::default(), Default::default())[0].identity,
        hits[1].identity
    );
    let replacement = iptv_search::ChannelIndex::from_names(["Generated", "Generated"])
        .search_channels("generated", &Default::default(), Default::default());
    assert!(
        replacement
            .iter()
            .all(|hit| hits.iter().all(|old| old.identity != hit.identity))
    );
    state.set_results("generated", replacement.clone(), Vec::new());
    assert_eq!(
        state.focus(),
        &Focus::Result(ResultId::Channel(replacement[0].identity.clone()))
    );
}

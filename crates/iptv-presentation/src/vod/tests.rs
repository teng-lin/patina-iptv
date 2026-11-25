use super::*;
use iptv_core::vod::{DISCOVERY_PAGE, Episode, Progress};
fn card(id: &str) -> Card {
    Card {
        key: Key {
            source: 1,
            kind: Kind::Movie,
            id: id.into(),
        },
        name: id.into(),
        source_name: String::new(),
        categories: vec![],
        poster: None,
        plot: None,
        year: None,
        rating_x10: None,
        duration_secs: None,
        progress: Progress::default(),
    }
}
#[test]
fn independent_sources_do_not_share_identity() {
    let a = card("same");
    let mut b = a.clone();
    b.key.source = 2;
    let mut s = Screen::default();
    let g = s.begin(Kind::Movie);
    s.publish(g, vec![a, b], false, false);
    assert_eq!(s.cards.len(), 2);
}

#[test]
fn refresh_restores_content_identity_and_rejects_old_results() {
    let mut s = Screen::default();
    let g = s.begin(Kind::Movie);
    s.publish(g, vec![card("a"), card("b")], false, false);
    s.move_focus(1);
    let g = s.reload();
    assert!(!s.publish(g - 1, vec![], false, false));
    s.publish(g, vec![card("b"), card("a")], false, false);
    assert_eq!(s.focused, Some(card("b").key));
}
#[test]
fn poster_grid_moves_in_rows_then_reveals_categories() {
    let mut s = Screen::default();
    let g = s.begin(Kind::Movie);
    s.publish(
        g,
        (0..10).map(|n| card(&n.to_string())).collect(),
        false,
        false,
    );
    s.columns = 4;
    s.browse_horizontal(1);
    assert!(!s.categories_open);
    s.browse_horizontal(1);
    assert_eq!(s.focused_index(), Some(1));
    s.browse_vertical(1);
    assert_eq!(s.focused_index(), Some(5));
    s.browse_vertical(1);
    assert_eq!(s.focused_index(), Some(9));
    s.browse_horizontal(-1);
    s.browse_horizontal(-1);
    assert!(s.categories_open);
    assert_eq!(s.focused_index(), Some(8));
    assert_eq!(s.control, Control::Category);
}
#[test]
fn series_actions_follow_the_visible_row_and_vertical_moves_reach_episodes() {
    let mut s = Screen::default();
    s.show_details(series_details(3));
    for control in [
        Control::Restart,
        Control::Next,
        Control::Block,
        Control::Block,
    ] {
        s.detail_horizontal(1);
        assert_eq!(s.control, control);
    }
    s.detail_vertical(1);
    assert_eq!(s.control, Control::Seasons);
    s.detail_vertical(1);
    assert_eq!(s.control, Control::Episodes);
    let before = s.episode.clone();
    s.detail_horizontal(1);
    assert_ne!(s.episode, before);
    s.detail_vertical(-1);
    assert_eq!(s.control, Control::Seasons);
    s.detail_vertical(-1);
    assert_eq!(s.control, Control::Play);
}
#[test]
fn movie_vertical_keys_keep_the_action_row_and_retry_is_horizontal() {
    let mut s = Screen::default();
    let mut details = series_details(0);
    details.item.key.kind = Kind::Movie;
    s.show_details(details);
    let key = s.details.as_ref().unwrap().item.key.clone();
    s.fail("Playback unavailable", Operation::Details(key));
    s.control = Control::Play;
    for step in [-1, 1] {
        s.detail_vertical(step);
        assert_eq!(s.control, Control::Play);
    }
    s.detail_horizontal(-1);
    assert_eq!(s.control, Control::Retry);
    s.detail_vertical(1);
    assert_eq!(s.control, Control::Retry);
    s.detail_horizontal(1);
    assert_eq!(s.control, Control::Play);
}
#[test]
fn series_retry_shares_the_action_row_above_seasons() {
    let mut s = Screen::default();
    s.show_details(series_details(2));
    let key = s.details.as_ref().unwrap().item.key.clone();
    s.fail("Playback unavailable", Operation::Details(key));
    s.control = Control::Play;
    s.detail_vertical(-1);
    assert_eq!(s.control, Control::Play);
    s.detail_horizontal(-1);
    assert_eq!(s.control, Control::Retry);
    s.detail_vertical(1);
    assert_eq!(s.control, Control::Seasons);
}
#[test]
fn revoked_metadata_is_removed_from_the_preview_and_catalog() {
    let mut s = Screen::default();
    let g = s.begin(Kind::Movie);
    let mut item = card("a");
    item.plot = Some("Generated protected plot".into());
    item.poster = Some("https://example.test/poster.png".into());
    s.publish(g, vec![item.clone()], false, false);
    s.publish_preview(
        g,
        Details {
            incarnation: "generated".into(),
            item: item.clone(),
            episodes: vec![],
            resume_episode: None,
        },
    );
    s.redact_metadata(&[item.key]);
    assert!(s.preview.is_none());
    assert!(s.cards[0].plot.is_none());
    assert!(s.cards[0].poster.is_none());
}
#[test]
fn preview_metadata_cannot_replace_another_title_or_a_newer_query() {
    let mut s = Screen::default();
    let g = s.begin(Kind::Movie);
    s.publish(g, vec![card("a"), card("b")], false, false);
    let details = |id| Details {
        incarnation: "generated".into(),
        item: card(id),
        episodes: vec![],
        resume_episode: None,
    };
    assert!(s.publish_preview(g, details("a")));
    s.move_focus(1);
    assert!(!s.publish_preview(g, details("a")));
    let next = s.reload();
    assert!(s.preview.is_none());
    assert!(!s.publish_preview(g, details("b")));
    s.publish(next, vec![card("b")], false, false);
    assert!(s.publish_preview(next, details("b")));
}
#[test]
fn down_at_a_page_boundary_waits_for_the_same_column_in_the_next_row() {
    let mut s = Screen::default();
    let g = s.begin(Kind::Movie);
    s.publish(
        g,
        (0..20).map(|n| card(&n.to_string())).collect(),
        true,
        false,
    );
    s.columns = 5;
    s.focused = Some(card("15").key);
    s.control = Control::Catalog;
    s.browse_vertical(1);
    assert_eq!(s.focused, Some(card("15").key));
    s.begin_more();
    s.publish(
        g,
        (20..25).map(|n| card(&n.to_string())).collect(),
        false,
        true,
    );
    assert_eq!(s.focused, Some(card("20").key));
}
#[test]
fn backing_away_from_a_pending_title_invalidates_its_reply() {
    let mut s = Screen::default();
    let g = s.begin(Kind::Movie);
    s.publish(g, vec![card("a")], false, false);
    s.browse_horizontal(1);
    let pending = s.begin_operation();
    s.back();
    assert!(s.open && s.categories_open);
    assert!(!s.busy);
    assert_ne!(s.generation, pending);
    assert!(!s.publish(pending, vec![card("old")], false, false));
    assert_eq!(s.focused, Some(card("a").key));
}
#[test]
fn a_remote_can_return_to_retry_after_a_failed_title() {
    let mut s = Screen::default();
    let g = s.begin(Kind::Movie);
    s.publish(g, vec![card("a")], false, false);
    s.browse_horizontal(1);
    s.fail("Generated failure", Operation::Details(card("a").key));
    s.browse_vertical(-1);
    assert_eq!(s.control, Control::Retry);
    s.browse_horizontal(1);
    assert_eq!(s.control, Control::Search);
    s.browse_horizontal(-1);
    assert_eq!(s.control, Control::Retry);
}
#[test]
fn category_previews_keep_source_qualified_identity() {
    let mut s = Screen::default();
    s.begin(Kind::Series);
    s.categories = [1, 2]
        .into_iter()
        .map(|source| Category {
            source,
            id: "same".into(),
            name: "Generated".into(),
            adult: false,
        })
        .collect();
    assert!(s.browse_vertical(1));
    assert_eq!(s.query.category, Some((1, "same".into())));
    assert!(s.browse_vertical(1));
    assert_eq!(s.query.category, Some((2, "same".into())));
    assert!(!s.browse_vertical(1));
    assert!(s.browse_vertical(-1));
    assert_eq!(s.query.category, Some((1, "same".into())));
}
#[test]
fn back_walks_from_posters_to_categories_to_navigation_before_leaving() {
    let mut s = Screen::default();
    s.begin(Kind::Movie);
    s.browse_horizontal(1);
    s.back();
    assert!(s.open && s.categories_open);
    assert_eq!(s.control, Control::Category);
    s.back();
    assert!(s.open);
    assert_eq!(s.control, Control::Navigation);
    s.back();
    assert!(!s.open);
}

fn keyed(source: i64, kind: Kind, id: &str) -> Card {
    let mut c = card(id);
    c.key.source = source;
    c.key.kind = kind;
    c
}

#[test]
fn a_shelf_mixes_kinds_and_sources_as_separate_identities() {
    let mut s = Screen::default();
    let g = s.begin_shelf(Shelf::ContinueWatching);
    assert!(s.is_shelf() && s.open && s.busy);
    let cards = vec![
        keyed(1, Kind::Movie, "same"),
        keyed(1, Kind::Series, "same"),
        keyed(2, Kind::Movie, "same"),
        keyed(2, Kind::Series, "same"),
    ];
    assert!(s.publish(g, cards.clone(), true, false));
    assert_eq!(s.cards.len(), 4, "kind and source keep duplicates apart");
    s.move_focus(2);
    assert_eq!(s.focused, Some(cards[2].key.clone()));
    // A refresh reorders the shelf (newest progress first): the focus
    // follows the title, not its row.
    let g = s.reload();
    let mut reordered = cards.clone();
    reordered.reverse();
    s.publish(g, reordered, false, false);
    assert_eq!(s.focused, Some(cards[2].key.clone()));
    // The next page repeats nothing already shown.
    s.publish(
        g,
        vec![keyed(2, Kind::Movie, "same"), keyed(3, Kind::Movie, "same")],
        false,
        true,
    );
    assert_eq!(s.cards.len(), 5);
}
#[test]
fn a_shelf_offers_only_its_titles_until_an_error_offers_retry() {
    let mut s = Screen::default();
    let g = s.begin_shelf(Shelf::RecentlyAdded);
    assert!(s.controls().is_empty());
    s.move_control(-1);
    assert_eq!(s.control, Control::Catalog, "no hidden toolbar to reach");
    s.fail("On-demand library could not be loaded", Operation::List);
    assert!(!s.busy);
    assert_eq!(s.control, Control::Retry, "OK repeats the failed load");
    assert_eq!(s.controls(), vec![Control::Retry]);
    assert_eq!(s.retry_operation(), Some(Operation::List));
    let g2 = s.reload();
    assert!(g2 != g && s.error.is_none() && s.retry.is_none());
    assert_eq!(s.control, Control::Catalog);
    s.publish(g2, vec![keyed(1, Kind::Movie, "a")], false, false);
    assert_eq!(s.controls(), vec![Control::Catalog]);
    // An error over a list keeps the focus on the titles.
    s.fail(
        "Details unavailable; refresh the playlist and try again",
        Operation::Details(keyed(1, Kind::Movie, "a").key),
    );
    assert_eq!(s.control, Control::Catalog);
    s.move_control(-1);
    assert_eq!(s.control, Control::Retry);
    s.move_control(1);
    assert_eq!(
        s.control,
        Control::Catalog,
        "populated cards remain reachable"
    );
    s.move_control(-1);
    s.back();
    assert_eq!(s.control, Control::Catalog);
    assert!(!s.open, "Back from the list closes the shelf");
}
#[test]
fn empty_shelf_navigation_stays_on_retry_and_empty_success_has_no_targets() {
    for shelf in [Shelf::ContinueWatching, Shelf::RecentlyAdded] {
        let mut screen = Screen::default();
        let generation = screen.begin_shelf(shelf);
        assert!(screen.publish(generation, Vec::new(), false, false));
        assert!(screen.controls().is_empty());
        for delta in [-1, 1] {
            screen.move_control(delta);
            assert_eq!(screen.control, Control::Catalog);
            assert!(screen.focused.is_none());
        }
        screen.fail("On-demand library could not be loaded", Operation::List);
        assert_eq!(screen.controls(), [Control::Retry]);
        for stale in [Control::Retry, Control::Catalog] {
            for delta in [-1, 1] {
                screen.control = stale;
                screen.move_control(delta);
                assert_eq!(
                    screen.control,
                    Control::Retry,
                    "empty errors offer only visible Retry"
                );
            }
        }
        screen.back();
        assert!(!screen.open, "empty shelves still dismiss with Back");
    }
}

#[test]
fn reopening_a_shelf_returns_to_its_title_by_identity() {
    let mut s = Screen::default();
    let g = s.begin_shelf(Shelf::ContinueWatching);
    let cards = vec![keyed(1, Kind::Movie, "a"), keyed(2, Kind::Series, "b")];
    s.publish(g, cards.clone(), false, false);
    s.move_focus(1);
    // Playback closed the shelf; the title moved to the top meanwhile.
    s.open = false;
    let g = s.begin_shelf(Shelf::ContinueWatching);
    s.publish(g, vec![cards[1].clone(), cards[0].clone()], false, false);
    assert_eq!(s.focused, Some(cards[1].key.clone()));
    // A finished title has left: the focus falls to the first one.
    let g = s.begin_shelf(Shelf::ContinueWatching);
    s.publish(g, vec![cards[0].clone()], false, false);
    assert_eq!(s.focused, Some(cards[0].key.clone()));
    // Another shelf or a catalog starts from its own top.
    let g = s.begin_shelf(Shelf::RecentlyAdded);
    assert_eq!(s.focused, None);
    s.publish(g, cards.clone(), false, false);
    s.move_focus(1);
    s.begin(Kind::Movie);
    assert_eq!(s.focused, None);
}
#[test]
fn back_from_details_returns_to_the_same_shelf_title() {
    let mut s = Screen::default();
    let g = s.begin_shelf(Shelf::ContinueWatching);
    let cards = vec![keyed(1, Kind::Movie, "a"), keyed(2, Kind::Series, "b")];
    s.publish(g, cards.clone(), false, false);
    s.show_details(Details {
        incarnation: "generated".into(),
        item: cards[1].clone(),
        episodes: vec![],
        resume_episode: None,
    });
    assert_eq!(s.control, Control::Play);
    assert!(s.controls().contains(&Control::Seasons));
    s.back();
    s.control = Control::Catalog;
    assert!(s.open && s.details.is_none() && s.is_shelf());
    assert_eq!(s.focused, Some(cards[1].key.clone()));
    assert_eq!(s.controls(), vec![Control::Catalog]);
}
fn numbered(count: usize) -> Vec<Card> {
    (0..count)
        .map(|i| {
            keyed(
                1 + (i % 2) as i64,
                Kind::Movie,
                &format!("generated-{i:03}"),
            )
        })
        .collect()
}
fn series_details(episodes: usize) -> Details {
    let item = keyed(1, Kind::Series, "generated-series");
    Details {
        incarnation: "generated".into(),
        episodes: (1..=episodes as u32)
            .map(|number| Episode {
                key: Key {
                    source: 1,
                    kind: Kind::Episode,
                    id: format!("generated-series:{number}"),
                },
                season: 1,
                number,
                title: format!("Generated episode {number}"),
                plot: None,
                duration_secs: Some(120),
                progress: Progress::default(),
            })
            .collect(),
        item,
        resume_episode: None,
    }
}
#[test]
fn retry_after_a_details_failure_opens_the_same_title_again() {
    let mut s = Screen::default();
    let g = s.begin_shelf(Shelf::ContinueWatching);
    let cards = numbered(3);
    s.publish(g, cards.clone(), false, false);
    s.move_focus(2);
    // Choosing a title: its details fail before anything opened.
    s.begin_operation();
    s.fail(
        "Details unavailable; refresh the playlist and try again",
        Operation::Details(cards[2].key.clone()),
    );
    assert!(s.details.is_none() && !s.busy);
    assert_eq!(
        s.retry_operation(),
        Some(Operation::Details(cards[2].key.clone())),
        "Retry opens the title again, not the list's first page"
    );
    assert_eq!(s.focused, Some(cards[2].key.clone()));
    assert_eq!(s.cards, cards, "the list is not reloaded");
    assert!(s.controls().contains(&Control::Retry));
    // Retrying starts the same operation: the error goes, and only a
    // newer reply is accepted.
    let before = s.generation;
    let again = s.begin_operation();
    assert!(again != before && s.error.is_none() && s.retry.is_none());
    s.show_details(series_details(2));
    assert_eq!(s.control, Control::Play);
}
#[test]
fn the_last_episode_skips_the_unmounted_next_action() {
    let mut screen = Screen::default();
    screen.begin_shelf(Shelf::ContinueWatching);
    let details = series_details(2);
    screen.show_details(details.clone());
    screen.episode = Some(details.episodes[1].key.clone());
    screen.control = Control::Restart;
    screen.detail_horizontal(1);
    assert_eq!(
        screen.control,
        Control::Block,
        "Right must reach a visible action, not the absent Next button"
    );
    screen.detail_horizontal(-1);
    assert_eq!(screen.control, Control::Restart);
    screen.episode = Some(details.episodes[0].key.clone());
    screen.detail_horizontal(1);
    assert_eq!(
        screen.control,
        Control::Next,
        "Next remains reachable when usable"
    );
}
#[test]
fn load_more_advances_after_a_refresh_repeats_a_whole_page() {
    let shelf = Shelf::RecentlyAdded;
    let original = numbered(120);
    let mut screen = Screen::default();
    let generation = screen.begin_shelf(shelf);
    screen.publish_window(generation, read(&original, DiscoveryQuery::first(shelf)));
    screen.move_focus(39);
    let focused = screen.focused.clone();
    // Forty new additions move the already shown titles onto the next
    // page. Deduplication must not make Load more request it forever.
    let mut refreshed: Vec<_> = (0..40)
        .map(|n| card(&format!("generated-new-{n:03}")))
        .collect();
    refreshed.extend(original.clone());
    for expected_offset in [40, 80, 120] {
        let offset = screen.begin_more();
        assert_eq!(offset, expected_offset);
        let page = read(&refreshed, DiscoveryQuery::after(shelf, offset as usize));
        assert!(screen.publish(generation, page.cards, page.more, true));
        assert_eq!(screen.focused, focused);
        if offset == 40 {
            assert_eq!(screen.cards.len(), 40, "the repeated page is deduplicated");
            assert!(screen.has_more);
        }
    }
    assert_eq!(screen.cards, original);
    assert!(!screen.has_more, "all remaining titles were reached");
}
#[test]
fn retry_after_a_later_page_fails_keeps_the_pages_and_focus() {
    let mut s = Screen::default();
    let g = s.begin_shelf(Shelf::RecentlyAdded);
    let all = numbered(100);
    s.publish(g, all[..40].to_vec(), true, false);
    s.move_focus(39);
    let focused = s.focused.clone();
    let offset = s.begin_more();
    assert_eq!(offset, 40);
    assert!(s.busy);
    s.fail(
        "On-demand library could not be loaded",
        Operation::More { offset },
    );
    assert_eq!(s.cards.len(), 40, "loaded pages stay");
    assert_eq!(s.focused, focused);
    assert_eq!(s.control, Control::Catalog, "the focus stays on the titles");
    assert!(s.has_more);
    assert_eq!(s.retry_operation(), Some(Operation::More { offset: 40 }));
    // Retry reads the same next page and appends it.
    assert_eq!(s.begin_more(), 40);
    assert!(s.error.is_none());
    assert!(s.publish(g, all[40..80].to_vec(), true, true));
    assert_eq!(s.cards.len(), 80);
    assert_eq!(s.focused, focused);
}
#[test]
fn retry_after_playback_fails_plays_the_same_episode_and_restart() {
    let mut s = Screen::default();
    let g = s.begin_shelf(Shelf::ContinueWatching);
    let mut details = series_details(4);
    s.publish(g, vec![details.item.clone()], false, false);
    s.show_details(details.clone());
    assert_eq!(s.episode, Some(details.episodes[0].key.clone()));
    // Down twice chooses the third episode; Restart plays it.
    s.move_episode(2);
    let third = details.episodes[2].key.clone();
    assert_eq!(s.episode, Some(third.clone()));
    s.control = Control::Restart;
    s.begin_operation();
    let failed = Operation::Play {
        item: details.item.key.clone(),
        key: third.clone(),
        restart: true,
        incarnation: details.incarnation.clone(),
    };
    s.fail(
        "Playback unavailable; refresh the playlist and try again",
        failed.clone(),
    );
    assert_eq!(s.retry_operation(), Some(failed));
    assert_eq!(s.episode, Some(third.clone()), "the chosen episode stays");
    assert_eq!(s.control, Control::Restart);
    assert!(s.controls().starts_with(&[Control::Retry, Control::Play]));
    // Retry reads fresh details first (their own preference is another
    // episode) and still plays the third episode from its start.
    s.begin_operation();
    details.resume_episode = Some(details.episodes[1].key.clone());
    details.episodes[1].progress = Progress::observed(30_000, Some(120_000), false);
    let chosen = details.incarnation.clone();
    assert_eq!(
        s.show_details_keeping(details.clone(), &third, &chosen),
        Retried::Ready
    );
    assert_eq!(s.episode, Some(third.clone()));
    assert_eq!(s.control, Control::Restart);
    // A refresh that removed the episode cannot play it.
    details.episodes.remove(2);
    assert_eq!(
        s.show_details_keeping(details.clone(), &third, &chosen),
        Retried::EpisodeGone
    );
    assert_eq!(s.episode, Some(details.episodes[1].key.clone()));
    // A movie keeps its own identity.
    let movie = Details {
        item: keyed(2, Kind::Movie, "generated-film"),
        episodes: vec![],
        ..details
    };
    let key = movie.item.key.clone();
    assert_eq!(s.show_details_keeping(movie, &key, &chosen), Retried::Ready);
    assert_eq!(s.episode, None);
}
#[test]
fn retried_playback_never_rebinds_to_a_recreated_or_edited_playlist() {
    let mut s = Screen::default();
    let g = s.begin_shelf(Shelf::ContinueWatching);
    let details = series_details(3);
    s.publish(g, vec![details.item.clone()], false, false);
    s.show_details(details.clone());
    let second = details.episodes[1].key.clone();
    s.episode = Some(second.clone());
    s.begin_operation();
    s.fail(
        "Playback unavailable; refresh the playlist and try again",
        Operation::Play {
            item: details.item.key.clone(),
            key: second.clone(),
            restart: false,
            incarnation: details.incarnation.clone(),
        },
    );
    let Some(Operation::Play { incarnation, .. }) = s.retry_operation() else {
        panic!("playback is retried as playback");
    };
    // Meanwhile the playlist was deleted and added again (or its
    // credentials edited) under the same id: the same keys, a new
    // incarnation. Its fresh details are not this title.
    s.begin_operation();
    let recreated = Details {
        incarnation: "generated-recreated".into(),
        ..details.clone()
    };
    assert_eq!(
        s.show_details_keeping(recreated, &second, &incarnation),
        Retried::PlaylistChanged
    );
    assert!(
        s.details.is_none(),
        "nothing from the new playlist is shown"
    );
    assert_eq!(s.episode, None);
    assert_eq!(s.control, Control::Catalog);
    assert_eq!(
        s.focused,
        Some(details.item.key.clone()),
        "the title stays focused, to be chosen again"
    );
}
/// A generated shelf order and the page a [`DiscoveryQuery`] reads from
/// it, as the store would (the store tests check the store itself).
fn read(order: &[Card], query: DiscoveryQuery) -> DiscoveryPage {
    let start = (query.offset as usize).min(order.len());
    let end = (start + query.bounded_limit() as usize).min(order.len());
    DiscoveryPage {
        cards: order[start..end].to_vec(),
        more: end < order.len(),
        start: query.offset,
    }
}
#[test]
fn reopening_a_shelf_shows_the_window_around_a_deep_title() {
    let shelf = Shelf::RecentlyAdded;
    let order = numbered(6_000);
    let target = order[5_000].key.clone();
    let mut s = Screen::default();
    let g = s.begin_shelf(shelf);
    s.publish(
        g,
        read(&order, DiscoveryQuery::first(shelf)).cards,
        true,
        false,
    );
    s.focused = Some(target.clone());
    // Playback or Back closed the shelf; reopening it keeps the title
    // and reads the one bounded page holding its current place.
    s.open = false;
    let g = s.begin_shelf(shelf);
    assert_eq!(s.focused, Some(target.clone()));
    let window = DiscoveryQuery::around(shelf, Some(5_000));
    assert_eq!(
        (window.offset, window.bounded_limit()),
        (5_000, DISCOVERY_PAGE)
    );
    assert!(s.publish_window(g, read(&order, window)));
    assert_eq!(s.focused, Some(target.clone()), "found by key, not row");
    assert_eq!(s.start, 5_000);
    assert_eq!(
        s.cards.len(),
        DISCOVERY_PAGE as usize,
        "no earlier page re-read"
    );
    assert!(s.has_previous());
    // Load more continues below the window, not after 40 titles.
    let offset = s.begin_more();
    assert_eq!(offset, 5_040);
    let below = read(&order, DiscoveryQuery::after(shelf, offset as usize));
    assert_eq!(below.cards[0].key, order[5_040].key);
    assert!(s.publish(g, below.cards, below.more, true));
    assert_eq!(s.start, 5_000);
    // Up on the first title reads the page above and steps onto it.
    s.focused = Some(order[5_000].key.clone());
    let above = s.begin_previous().unwrap();
    assert_eq!((above.offset, above.bounded_limit()), (4_960, 40));
    assert!(s.publish_previous(g, 5_000, read(&order, above), true));
    assert_eq!(s.start, 4_960);
    assert_eq!(s.cards[0].key, order[4_960].key);
    assert_eq!(s.focused, Some(order[4_999].key.clone()));
    // A failed previous page keeps the window; Retry reads it again.
    let above = s.begin_previous().unwrap();
    s.fail(
        "On-demand library could not be loaded",
        Operation::Previous {
            start: 4_960,
            step_up: false,
        },
    );
    assert_eq!((s.start, s.cards.len()), (4_960, 120));
    assert_eq!(s.focused, Some(order[4_999].key.clone()));
    assert_eq!(s.begin_previous(), Some(above));
    assert!(s.publish_previous(g, 4_960, read(&order, above), false));
    assert_eq!(s.start, 4_920);
    assert_eq!(
        s.focused,
        Some(order[4_999].key.clone()),
        "a button keeps focus"
    );
    // The shelf's top has nothing before it.
    let g = s.begin_shelf(shelf);
    assert!(s.publish_window(g, read(&order, DiscoveryQuery::around(shelf, Some(3)))));
    assert_eq!(s.start, 0);
    assert!(!s.has_previous() && s.begin_previous().is_none());
}
#[test]
fn a_reordered_or_deleted_deep_title_is_handled_by_identity() {
    let shelf = Shelf::ContinueWatching;
    let order = numbered(3_000);
    let target = order[2_500].key.clone();
    let mut s = Screen::default();
    let g = s.begin_shelf(shelf);
    s.publish(g, order[..40].to_vec(), true, false);
    s.focused = Some(target.clone());
    s.open = false;
    // Watched again meanwhile: the title moved from 2,501st to 2,123rd.
    let mut reordered = order.clone();
    let moved = reordered.remove(2_500);
    reordered.insert(2_122, moved);
    let g = s.begin_shelf(shelf);
    let window = DiscoveryQuery::around(shelf, Some(2_122));
    assert_eq!(window.offset, 2_120);
    assert!(s.publish_window(g, read(&reordered, window)));
    assert_eq!(s.focused, Some(target.clone()));
    assert_eq!(s.start, 2_120);
    // A failed window read keeps the focused title for Retry.
    s.open = false;
    let g = s.begin_shelf(shelf);
    s.fail("On-demand library could not be loaded", Operation::List);
    assert_eq!(s.focused, Some(target.clone()), "the anchor is not lost");
    assert_eq!(s.retry_operation(), Some(Operation::List));
    // Finished or deleted meanwhile: the shelf no longer lists it, so
    // the first page is shown with the focus on its first title.
    let mut gone = reordered.clone();
    gone.remove(2_122);
    let g2 = s.reload();
    assert!(g2 != g);
    assert_eq!(s.focused, Some(target.clone()));
    let first = DiscoveryQuery::around(shelf, None);
    assert_eq!(first, DiscoveryQuery::first(shelf));
    assert!(s.publish_window(g2, read(&gone, first)));
    assert_eq!(s.start, 0);
    assert_eq!(s.focused, Some(gone[0].key.clone()));
    // A catalog never windows.
    let g = s.begin(Kind::Movie);
    s.publish(g, numbered(3), false, false);
    assert_eq!(s.start, 0);
    assert!(!s.has_previous() && s.begin_previous().is_none());
}
#[test]
fn play_resumes_the_chosen_episode_not_the_series_record() {
    let mut details = series_details(3);
    // The second episode was finished last; the series record still
    // holds its end point, but the third episode starts from zero.
    details.item.progress = Progress {
        position_ms: 119_000,
        duration_ms: Some(120_000),
        completed: false,
    };
    details.resume_episode = Some(details.episodes[1].key.clone());
    details.episodes[1].progress = Progress::observed(120_000, Some(120_000), true);
    details.episodes[2].progress = Progress::observed(2_000, Some(120_000), false);
    let mut s = Screen::default();
    s.show_details(details.clone());
    assert_eq!(s.episode, Some(details.episodes[2].key.clone()));
    assert!(!s.resumes(), "the next episode plays from its start");
    details.episodes[2].progress = Progress::observed(45_000, Some(120_000), false);
    s.show_details(details);
    assert!(s.resumes());
    let mut film = keyed(2, Kind::Movie, "generated-film");
    film.progress = Progress::observed(30_000, Some(120_000), false);
    s.show_details(Details {
        incarnation: "generated".into(),
        item: film,
        episodes: vec![],
        resume_episode: None,
    });
    assert!(s.resumes());
}
#[test]
fn an_empty_catalog_enters_the_visible_toolbar_without_skipping_controls() {
    for kind in [Kind::Movie, Kind::Series] {
        let mut screen = Screen::default();
        let generation = screen.begin(kind);
        screen.publish(generation, Vec::new(), false, false);
        for (delta, expected) in [(-1, Control::Continue), (1, Control::Search)] {
            screen.control = Control::Catalog;
            screen.move_control(delta);
            assert_eq!(screen.control, expected, "{kind:?}, direction {delta}");
        }
        screen.move_control(1);
        assert_eq!(
            screen.control,
            Control::Source,
            "the next step advances normally"
        );
    }
}
#[test]
fn an_empty_unfiltered_catalog_offers_its_one_action_where_titles_would_be() {
    let mut s = Screen::default();
    s.set_library(VodState::Failed {
        source: 7,
        problem: ImportProblem::Network,
    });
    let generation = s.begin(Kind::Movie);
    assert_eq!(s.empty_action(), None, "nothing is offered while loading");
    assert!(
        matches!(s.library, VodState::Failed { source: 7, .. }),
        "kept across begin"
    );
    s.publish(generation, Vec::new(), false, false);
    assert_eq!(s.empty_action(), Some(EmptyAction::Retry(7)));
    assert!(s.controls().contains(&Control::Catalog));
    // A search or filter that finds nothing is not the library's state.
    s.query.text = "generated".into();
    assert_eq!(s.empty_state(), None);
    s.query.text.clear();
    s.query.continue_watching = true;
    assert_eq!(s.empty_action(), None);
    s.query.continue_watching = false;
    // Titles available elsewhere, a running import: no action.
    for state in [VodState::Available, VodState::Importing] {
        s.library = state;
        assert_eq!(s.empty_state(), Some(state));
        assert_eq!(s.empty_action(), None);
        assert!(!s.controls().contains(&Control::Catalog));
    }
    // A failed load shows Retry for the load, not the library's action.
    s.library = VodState::NoEligibleSource;
    s.fail("generated failure", Operation::List);
    assert_eq!(s.empty_action(), None);
    // Shelves keep their own empty text.
    let mut shelf = Screen::default();
    shelf.set_library(VodState::NoEligibleSource);
    let generation = shelf.begin_shelf(Shelf::RecentlyAdded);
    shelf.publish(generation, Vec::new(), false, false);
    assert_eq!(shelf.empty_state(), None);
}
#[test]
fn the_remote_leaves_an_action_that_moved_to_another_playlist() {
    let failed = |source| VodState::Failed {
        source,
        problem: ImportProblem::Network,
    };
    let mut s = Screen::default();
    s.set_library(failed(1));
    let generation = s.begin(Kind::Movie);
    s.publish(generation, Vec::new(), false, false);
    s.browse_horizontal(1);
    assert_eq!(s.control, Control::Catalog);
    assert_eq!(s.empty_action(), Some(EmptyAction::Retry(1)));
    // Retrying it: nothing offered meanwhile, the remote goes to the
    // categories (#146).
    s.set_library(VodState::Importing);
    assert_eq!(s.control, Control::Category);
    // The same playlist, now empty: Right reaches its Refresh.
    s.set_library(VodState::Empty { source: 1 });
    s.browse_horizontal(1);
    assert_eq!(s.control, Control::Catalog);
    assert_eq!(s.empty_action(), Some(EmptyAction::Refresh(1)));
    // Still offered by the same playlist: the remote stays.
    s.set_library(VodState::Empty { source: 1 });
    assert_eq!(s.control, Control::Catalog);
    // Another playlist's failure takes the screen: the remote leaves.
    s.set_library(failed(2));
    assert_eq!(s.control, Control::Category);
    assert!(s.categories_open);
    s.browse_horizontal(1);
    assert_eq!(s.empty_action(), Some(EmptyAction::Retry(2)));
    // A remote elsewhere is not moved.
    s.control = Control::Search;
    s.set_library(failed(3));
    assert_eq!(s.control, Control::Search);
    // An empty shelf keeps its focus.
    let mut shelf = Screen::default();
    let generation = shelf.begin_shelf(Shelf::RecentlyAdded);
    shelf.publish(generation, Vec::new(), false, false);
    shelf.set_library(failed(1));
    shelf.set_library(failed(2));
    assert_eq!(shelf.control, Control::Catalog);
    assert!(!shelf.categories_open);
}
#[test]
fn a_compact_empty_catalog_is_one_pane_between_the_rail_and_its_action() {
    let mut s = Screen::default();
    s.set_compact(true);
    s.set_library(VodState::NoEligibleSource);
    let generation = s.begin(Kind::Movie);
    // Loading: the categories show as before.
    assert!(!s.one_pane() && s.shows_categories());
    s.publish(generation, Vec::new(), false, false);
    // Loaded and empty: the message and its action take the width,
    // with the remote on the action and the rail beside it.
    assert!(s.one_pane());
    assert!(!s.shows_categories() && s.shows_navigation());
    assert_eq!(s.control, Control::Catalog);
    assert_eq!(s.empty_action(), Some(EmptyAction::AddPlaylist));
    // Left reaches the rail, Right the action again; the categories
    // never come back.
    s.browse_horizontal(-1);
    assert_eq!(s.control, Control::Navigation);
    assert!(!s.shows_categories() && s.shows_navigation());
    s.browse_horizontal(1);
    assert_eq!(s.control, Control::Catalog);
    // Choosing this destination again goes to its action.
    s.control = Control::Navigation;
    s.open_categories();
    assert_eq!(s.control, Control::Catalog);
    // Back: the rail, then closed.
    s.back();
    assert!(s.open);
    assert_eq!(s.control, Control::Navigation);
    assert!(s.shows_navigation());
    s.back();
    assert!(!s.open);
    // Wider again with the remote on the action: the categories one
    // Left away, as in every wider window; compact again moves a
    // remote off them.
    let generation = s.begin(Kind::Series);
    s.publish(generation, Vec::new(), false, false);
    s.set_compact(false);
    assert!(!s.one_pane() && !s.shows_navigation());
    s.browse_horizontal(-1);
    assert_eq!(s.control, Control::Category);
    assert!(s.shows_categories());
    s.set_compact(true);
    assert_eq!(s.control, Control::Catalog);
    assert!(!s.shows_categories());
    // Titles: the categories stay a pane of their own.
    let generation = s.begin(Kind::Movie);
    s.publish(generation, vec![card("a")], false, false);
    assert!(!s.one_pane() && s.shows_categories());
    assert_eq!(s.control, Control::Category);
    // A shelf is never one pane.
    let generation = s.begin_shelf(Shelf::RecentlyAdded);
    s.publish(generation, Vec::new(), false, false);
    assert!(!s.one_pane());
}
#[test]
fn the_rail_stays_on_screen_with_the_remote_when_one_pane_ends() {
    // Left to the rail, then a wider window or smaller text.
    let mut s = Screen::default();
    s.set_compact(true);
    s.set_library(VodState::NoEligibleSource);
    let generation = s.begin(Kind::Movie);
    s.publish(generation, Vec::new(), false, false);
    s.browse_horizontal(-1);
    s.set_compact(false);
    assert_eq!(s.control, Control::Navigation);
    assert!(s.shows_navigation() && s.shows_categories());
    s.browse_horizontal(1);
    assert_eq!(s.control, Control::Category);
    // Back to the rail, then wider.
    s.set_compact(true);
    assert_eq!(s.control, Control::Catalog);
    s.back();
    assert_eq!(s.control, Control::Navigation);
    s.set_compact(false);
    assert!(s.shows_navigation());
    // An import running offers no action: the remote waits on the rail,
    // not on an empty pane. It finishes with titles: the rail stays.
    let mut s = Screen::default();
    s.set_compact(true);
    s.set_library(VodState::Importing);
    let generation = s.begin(Kind::Movie);
    s.publish(generation, Vec::new(), false, false);
    assert!(s.one_pane());
    assert_eq!(s.control, Control::Navigation);
    assert!(s.shows_navigation() && !s.shows_categories());
    s.set_library(VodState::Available);
    let generation = s.reload();
    assert!(!s.one_pane());
    assert_eq!(s.control, Control::Navigation);
    assert!(s.shows_navigation());
    s.publish(generation, vec![card("a")], false, false);
    assert_eq!(s.control, Control::Navigation);
    assert!(s.shows_navigation() && s.shows_categories());
    s.browse_horizontal(1);
    assert_eq!(s.control, Control::Category);
}
#[test]
fn a_pending_read_never_lands_the_remote_on_another_playlists_action() {
    let mut s = Screen::default();
    s.set_compact(true);
    s.set_library(VodState::Failed {
        source: 1,
        problem: ImportProblem::Network,
    });
    let generation = s.begin(Kind::Movie);
    s.publish(generation, Vec::new(), false, false);
    assert_eq!(s.control, Control::Catalog);
    assert_eq!(s.empty_action(), Some(EmptyAction::Retry(1)));
    // A read is pending when the action moves to playlist 2: the
    // remote leaves, and the empty page does not put it back on 2's
    // action.
    let generation = s.reload();
    s.set_library(VodState::ImportsDisabled { source: 2 });
    assert_eq!(s.control, Control::Category);
    s.publish(generation, Vec::new(), false, false);
    assert!(s.one_pane());
    assert_eq!(s.control, Control::Navigation);
    assert!(s.shows_navigation());
    // Right from the rail, the action now shown, reaches it.
    s.browse_horizontal(1);
    assert_eq!(s.control, Control::Catalog);
    assert_eq!(s.empty_action(), Some(EmptyAction::PlaylistSettings(2)));
    // Shown: an action that moves again sends it to the rail.
    s.set_library(VodState::Empty { source: 3 });
    assert_eq!(s.control, Control::Navigation);
    assert!(s.shows_navigation() && !s.shows_categories());
}
#[test]
fn a_vanished_action_sends_the_remote_to_the_rail() {
    let mut s = Screen::default();
    s.set_compact(true);
    s.set_library(VodState::Empty { source: 1 });
    let generation = s.begin(Kind::Movie);
    s.publish(generation, Vec::new(), false, false);
    assert_eq!(s.control, Control::Catalog);
    // Refresh now: the import runs, the action goes.
    s.set_library(VodState::Importing);
    assert_eq!(s.empty_action(), None);
    assert_eq!(s.control, Control::Navigation);
    assert!(s.shows_navigation());
    // The same while a read is pending: settled when it arrives.
    s.control = Control::Catalog;
    let generation = s.reload();
    s.publish(generation, Vec::new(), false, false);
    assert_eq!(s.control, Control::Navigation);
}
#[test]
fn a_failed_read_and_its_retry_never_arm_another_playlists_action() {
    for compact in [true, false] {
        let mut s = Screen::default();
        s.set_compact(compact);
        s.set_library(VodState::Failed {
            source: 1,
            problem: ImportProblem::Network,
        });
        let generation = s.begin(Kind::Movie);
        s.publish(generation, Vec::new(), false, false);
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Catalog);
        // The action moves to playlist 2 during a read, which fails.
        s.reload();
        s.set_library(VodState::ImportsDisabled { source: 2 });
        s.fail("generated failure", Operation::List);
        assert_eq!(s.control, Control::Retry);
        // Retry puts the remote where the titles would be; the empty
        // page then shows playlist 2's action, which it never saw.
        let generation = s.reload();
        assert_eq!(s.control, Control::Catalog);
        s.publish(generation, Vec::new(), false, false);
        assert_eq!(s.empty_action(), Some(EmptyAction::PlaylistSettings(2)));
        assert_ne!(s.control, Control::Catalog, "compact: {compact}");
        assert!(s.shows_navigation() || s.shows_categories());
        // Moving there on purpose reaches it.
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Catalog);
    }
}
#[test]
fn the_remote_reaches_the_toolbar_of_a_compact_import() {
    let mut s = Screen::default();
    s.set_compact(true);
    s.set_library(VodState::Importing);
    let generation = s.begin(Kind::Movie);
    s.publish(generation, Vec::new(), false, false);
    assert_eq!(s.control, Control::Navigation);
    // Right: the toolbar, not the rail again.
    s.browse_horizontal(1);
    assert_eq!(s.control, Control::Search);
    s.browse_horizontal(1);
    assert_eq!(s.control, Control::Sort);
    // Down has no empty pane to enter.
    s.browse_vertical(1);
    assert_eq!(s.control, Control::Sort);
    // Left from the toolbar's first control: the rail.
    s.browse_horizontal(-1);
    s.browse_horizontal(-1);
    assert_eq!(s.control, Control::Navigation);
    // With an action, Right reaches the action as before.
    s.set_library(VodState::NoEligibleSource);
    s.browse_horizontal(1);
    assert_eq!(s.control, Control::Catalog);
}
#[test]
fn a_target_change_on_retry_never_arms_the_new_action() {
    for compact in [true, false] {
        let mut s = Screen::default();
        s.set_compact(compact);
        s.set_library(VodState::Failed {
            source: 1,
            problem: ImportProblem::Network,
        });
        let generation = s.begin(Kind::Movie);
        s.publish(generation, Vec::new(), false, false);
        s.browse_horizontal(1);
        // The read fails first; Retry takes the remote.
        s.reload();
        s.fail("generated failure", Operation::List);
        assert_eq!(s.control, Control::Retry);
        // Then the action moves to playlist 2, and its reload empties.
        s.set_library(VodState::ImportsDisabled { source: 2 });
        assert_eq!(s.control, Control::Retry);
        let generation = s.reload();
        s.publish(generation, Vec::new(), false, false);
        assert_eq!(s.empty_action(), Some(EmptyAction::PlaylistSettings(2)));
        assert_ne!(s.control, Control::Catalog, "compact: {compact}");
    }
}
#[test]
fn ok_on_a_category_enters_a_shown_action_for_good() {
    let mut s = Screen::default();
    s.set_library(VodState::Failed {
        source: 1,
        problem: ImportProblem::Network,
    });
    let generation = s.begin(Kind::Movie);
    s.publish(generation, Vec::new(), false, false);
    s.browse_horizontal(1);
    // Wide: the action moves to playlist 2, the remote goes to the
    // categories; OK there enters 2's shown action.
    s.set_library(VodState::ImportsDisabled { source: 2 });
    assert_eq!(s.control, Control::Category);
    s.enter_catalog();
    assert_eq!(s.control, Control::Catalog);
    // Compact afterwards keeps it there.
    s.set_compact(true);
    assert_eq!(s.control, Control::Catalog);
}
#[test]
fn closing_search_in_a_compact_import_keeps_the_remote_on_search() {
    let mut s = Screen::default();
    s.set_compact(true);
    s.set_library(VodState::Importing);
    let generation = s.begin(Kind::Movie);
    s.publish(generation, Vec::new(), false, false);
    s.browse_horizontal(1);
    assert_eq!(s.control, Control::Search);
    s.search_open = true;
    s.back();
    assert!(s.open && !s.search_open);
    assert_eq!(s.control, Control::Search);
    s.search_open = true;
    s.close_search();
    assert_eq!(s.control, Control::Search);
    // With an action, closing search goes where the titles would be.
    s.set_library(VodState::NoEligibleSource);
    s.search_open = true;
    s.close_search();
    assert_eq!(s.control, Control::Catalog);
}
#[test]
fn a_compact_move_to_the_rail_survives_a_failed_read_and_retry() {
    let mut s = Screen::default();
    s.set_compact(true);
    s.set_library(VodState::Failed {
        source: 1,
        problem: ImportProblem::Network,
    });
    let generation = s.begin(Kind::Movie);
    s.publish(generation, Vec::new(), false, false);
    assert_eq!(s.control, Control::Catalog);
    // Loaded and shown: the action moves to playlist 2, the remote to
    // the rail. The next read fails; Retry, then an empty page.
    s.set_library(VodState::ImportsDisabled { source: 2 });
    assert_eq!(s.control, Control::Navigation);
    s.reload();
    s.fail("generated failure", Operation::List);
    let generation = s.reload();
    assert_eq!(s.control, Control::Catalog);
    s.publish(generation, Vec::new(), false, false);
    assert_ne!(s.control, Control::Catalog);
    // Right from the rail selects the shown action.
    s.browse_horizontal(1);
    assert_eq!(s.control, Control::Catalog);
    assert_eq!(s.empty_action(), Some(EmptyAction::PlaylistSettings(2)));
}
#[test]
fn a_key_that_leaves_retry_focused_keeps_the_mark() {
    for compact in [true, false] {
        let mut s = Screen::default();
        s.set_compact(compact);
        s.set_library(VodState::Failed {
            source: 1,
            problem: ImportProblem::Network,
        });
        let generation = s.begin(Kind::Movie);
        s.publish(generation, Vec::new(), false, false);
        s.browse_horizontal(1);
        s.reload();
        s.fail("generated failure", Operation::List);
        s.set_library(VodState::ImportsDisabled { source: 2 });
        // Up (and Left) on Retry: the focus stays.
        s.browse_vertical(-1);
        s.browse_horizontal(-1);
        assert_eq!(s.control, Control::Retry);
        let generation = s.reload();
        s.publish(generation, Vec::new(), false, false);
        assert_ne!(s.control, Control::Catalog, "compact: {compact}");
    }
}
#[test]
fn a_wide_refresh_leaves_the_remote_on_a_visible_control() {
    // #146: Refresh now in a wider window, then the import runs.
    let mut s = Screen::default();
    s.set_library(VodState::Empty { source: 1 });
    let generation = s.begin(Kind::Movie);
    s.publish(generation, Vec::new(), false, false);
    s.browse_horizontal(1);
    assert_eq!(s.control, Control::Catalog);
    assert!(!s.shows_categories());
    s.set_library(VodState::Importing);
    assert_eq!(s.empty_action(), None);
    assert_eq!(s.control, Control::Category);
    assert!(s.shows_categories() && s.shows_navigation());
    // The same when the read showing it arrives later.
    s.control = Control::Catalog;
    s.categories_open = false;
    let generation = s.reload();
    assert_eq!(s.control, Control::Catalog, "loading: left alone");
    s.publish(generation, Vec::new(), false, false);
    assert_eq!(s.control, Control::Category);
    assert!(s.shows_categories());
    // The import ends empty again: Right reaches the shown action.
    s.set_library(VodState::Empty { source: 1 });
    assert_eq!(s.control, Control::Category);
    s.browse_horizontal(1);
    assert_eq!(s.control, Control::Catalog);
    assert_eq!(s.empty_action(), Some(EmptyAction::Refresh(1)));
    // It ends with titles: the remote stays on the categories shown
    // with them, and Right enters the posters.
    s.set_library(VodState::Available);
    assert_eq!(s.control, Control::Category);
    let generation = s.reload();
    s.publish(generation, vec![card("a")], false, false);
    assert_eq!(s.control, Control::Category);
    assert!(s.shows_categories());
    s.browse_horizontal(1);
    assert_eq!(s.control, Control::Catalog);
    assert_eq!(s.focused, Some(card("a").key));
}
#[test]
fn wide_arrows_never_land_on_an_empty_pane_without_an_action() {
    for state in [VodState::Importing, VodState::Available] {
        let mut s = Screen::default();
        s.set_library(state);
        let generation = s.begin(Kind::Series);
        s.publish(generation, Vec::new(), false, false);
        assert_eq!(s.control, Control::Category, "{state:?}");
        // OK on a category has nothing to enter.
        s.enter_catalog();
        assert_eq!(s.control, Control::Category, "{state:?}");
        // Right: the toolbar, not the empty pane.
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Search, "{state:?}");
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Sort, "{state:?}");
        // Down has no empty pane to enter.
        s.browse_vertical(1);
        assert_eq!(s.control, Control::Sort, "{state:?}");
        s.browse_horizontal(-1);
        s.browse_vertical(1);
        assert_eq!(s.control, Control::Search, "{state:?}");
        // Closing Search keeps the remote there.
        s.search_open = true;
        s.back();
        assert!(s.open && !s.search_open);
        assert_eq!(s.control, Control::Search, "{state:?}");
        // Left from the toolbar's first control: the categories.
        s.browse_horizontal(-1);
        assert_eq!(s.control, Control::Category, "{state:?}");
        assert!(s.shows_categories());
        // Once an action shows, Right and Down reach it.
        s.set_library(VodState::NoEligibleSource);
        assert_eq!(s.control, Control::Category, "{state:?}");
        s.browse_horizontal(1);
        assert_eq!(s.control, Control::Catalog, "{state:?}");
        s.control = Control::Search;
        s.browse_vertical(1);
        assert_eq!(s.control, Control::Catalog, "{state:?}");
        s.control = Control::Search;
        s.search_open = true;
        s.close_search();
        assert_eq!(s.control, Control::Catalog, "{state:?}");
        s.control = Control::Category;
        s.enter_catalog();
        assert_eq!(s.control, Control::Catalog, "{state:?}");
    }
    // A search or filter that finds nothing keeps its routes.
    let mut s = Screen::default();
    s.set_library(VodState::Importing);
    let generation = s.begin(Kind::Movie);
    s.query.text = "generated".into();
    s.publish(generation, Vec::new(), false, false);
    s.browse_horizontal(1);
    assert_eq!(s.control, Control::Catalog);
    // Titles: Right enters them, Left from Search stays.
    let mut s = Screen::default();
    s.set_library(VodState::Importing);
    let generation = s.begin(Kind::Movie);
    s.publish(generation, vec![card("a")], false, false);
    s.browse_horizontal(1);
    assert_eq!(s.control, Control::Catalog);
    s.control = Control::Search;
    s.browse_horizontal(-1);
    assert_eq!(s.control, Control::Search);
    s.browse_vertical(1);
    assert_eq!(s.control, Control::Catalog);
}
#[test]
fn a_wide_remote_sent_off_an_action_stays_off_through_an_import() {
    // The existing guard: the action moves to playlist 2 while a read
    // is pending; an import then hides it, then 2's action shows.
    let mut s = Screen::default();
    s.set_library(VodState::Failed {
        source: 1,
        problem: ImportProblem::Network,
    });
    let generation = s.begin(Kind::Movie);
    s.publish(generation, Vec::new(), false, false);
    s.browse_horizontal(1);
    let generation = s.reload();
    s.set_library(VodState::ImportsDisabled { source: 2 });
    assert_eq!(s.control, Control::Category);
    s.publish(generation, Vec::new(), false, false);
    assert_eq!(s.control, Control::Category);
    s.set_library(VodState::Importing);
    assert_eq!(s.control, Control::Category);
    s.set_library(VodState::ImportsDisabled { source: 2 });
    assert_eq!(s.control, Control::Category);
    // Right reaches it on purpose.
    s.browse_horizontal(1);
    assert_eq!(s.control, Control::Catalog);
    assert_eq!(s.empty_action(), Some(EmptyAction::PlaylistSettings(2)));
}
#[test]
fn an_import_that_displaces_the_remote_keeps_compact_off_an_unseen_action() {
    // Retry playlist 1 in a wide window; its import hides the action
    // and moves the remote to the categories. It ends with no titles,
    // and playlist 2's "Playlist settings" now shows.
    let mut s = Screen::default();
    s.set_library(VodState::Failed {
        source: 1,
        problem: ImportProblem::Network,
    });
    let generation = s.begin(Kind::Movie);
    s.publish(generation, Vec::new(), false, false);
    s.browse_horizontal(1);
    assert_eq!(s.control, Control::Catalog);
    s.set_library(VodState::Importing);
    assert_eq!(s.control, Control::Category);
    s.set_library(VodState::ImportsDisabled { source: 2 });
    let generation = s.reload();
    s.publish(generation, Vec::new(), false, false);
    assert_eq!(s.empty_action(), Some(EmptyAction::PlaylistSettings(2)));
    assert_eq!(s.control, Control::Category);
    // Compact: the rail, not playlist 2's action.
    s.set_compact(true);
    assert_eq!(s.control, Control::Navigation);
    // Entering it on purpose arms it, and compact keeps it.
    s.set_compact(false);
    assert_eq!(s.control, Control::Navigation);
    s.browse_horizontal(1);
    assert_eq!(s.control, Control::Category);
    s.browse_horizontal(1);
    assert_eq!(s.control, Control::Catalog);
    s.set_compact(true);
    assert_eq!(s.control, Control::Catalog);
}
#[test]
fn closing_search_during_a_pending_read_waits_on_search() {
    for compact in [false, true] {
        // An import running: the search text cleared, then submitted
        // before its read replies.
        let mut s = Screen::default();
        s.set_compact(compact);
        s.set_library(VodState::Importing);
        let generation = s.begin(Kind::Movie);
        s.query.text = "generated".into();
        s.publish(generation, Vec::new(), false, false);
        s.control = Control::Search;
        s.search_open = true;
        s.query.text.clear();
        let generation = s.reload();
        s.close_search();
        assert_eq!(s.control, Control::Search, "compact: {compact}");
        s.publish(generation, Vec::new(), false, false);
        assert_eq!(s.control, Control::Search, "compact: {compact}");
        // The same with Back, which reads the list again.
        s.search_open = true;
        s.reload();
        s.back();
        assert!(s.open && !s.search_open);
        assert_eq!(s.control, Control::Search, "compact: {compact}");
        let generation = s.reload();
        s.publish(generation, Vec::new(), false, false);
        assert_eq!(s.control, Control::Search, "compact: {compact}");
        // A reply with titles: the remote goes to them.
        s.search_open = true;
        let generation = s.reload();
        s.close_search();
        assert_eq!(s.control, Control::Search, "compact: {compact}");
        s.publish(generation, vec![card("a")], false, false);
        assert_eq!(s.control, Control::Catalog, "compact: {compact}");
        // A reply with an action: the remote goes to it.
        s.set_library(VodState::NoEligibleSource);
        s.control = Control::Search;
        s.search_open = true;
        let generation = s.reload();
        s.close_search();
        s.publish(generation, Vec::new(), false, false);
        assert_eq!(s.control, Control::Catalog, "compact: {compact}");
        // Moved elsewhere meanwhile: the reply leaves it there.
        s.control = Control::Search;
        s.search_open = true;
        let generation = s.reload();
        s.close_search();
        s.control = Control::Sort;
        s.publish(generation, Vec::new(), false, false);
        assert_eq!(s.control, Control::Sort, "compact: {compact}");
    }
}
#[test]
fn back_from_search_with_titles_waits_for_the_pending_read() {
    // Titles shown, a search typed: Back while its read is pending
    // cancels it (the shell reads again) and waits on Search.
    let mut s = Screen::default();
    let generation = s.begin(Kind::Movie);
    s.publish(generation, vec![card("a")], false, false);
    s.control = Control::Search;
    s.open_search();
    s.query.text = "generated".into();
    s.reload();
    s.back();
    assert!(s.open && !s.search_open);
    assert_eq!(s.control, Control::Search);
    let generation = s.reload();
    s.publish(generation, vec![card("b")], false, false);
    assert_eq!(s.control, Control::Catalog);
    assert_eq!(s.focused, Some(card("b").key));
}
#[test]
fn a_new_search_is_never_left_by_an_earlier_close() {
    // Two ways back into the field: OK on Search, or typing (a read
    // while it is open).
    for reopen in [true, false] {
        let mut s = Screen::default();
        let generation = s.begin(Kind::Movie);
        s.publish(generation, vec![card("a")], false, false);
        s.control = Control::Search;
        s.open_search();
        s.query.text = "generated".into();
        s.reload();
        s.close_search();
        assert_eq!(s.control, Control::Search);
        // Back into the field before the reply; typing reads again.
        if reopen {
            s.open_search();
        } else {
            s.search_open = true;
        }
        s.query.text = "generated more".into();
        let generation = s.reload();
        s.publish(generation, vec![card("b")], false, false);
        assert!(s.search_open);
        assert_eq!(s.control, Control::Search, "reopen: {reopen}");
        // Even a stale wait never leaves an open field.
        s.search_closed_pending = true;
        let generation = s.reload();
        s.search_open = true;
        s.publish(generation, vec![card("b")], false, false);
        assert_eq!(s.control, Control::Search, "reopen: {reopen}");
    }
}
#[test]
fn closing_search_onto_a_shown_action_enters_it() {
    // The remote is sent off playlist 1's action when it moves to
    // playlist 2, then goes to Search (a pointer) and closes it with
    // playlist 2's action on screen.
    let mut s = Screen::default();
    s.set_library(VodState::Failed {
        source: 1,
        problem: ImportProblem::Network,
    });
    let generation = s.begin(Kind::Movie);
    s.publish(generation, Vec::new(), false, false);
    s.browse_horizontal(1);
    s.set_library(VodState::ImportsDisabled { source: 2 });
    assert_eq!(s.control, Control::Category);
    s.control = Control::Search;
    s.open_search();
    s.close_search();
    assert_eq!(s.control, Control::Catalog);
    // Entered on purpose: a resize or the next read keeps it there.
    s.set_compact(true);
    assert_eq!(s.control, Control::Catalog);
    s.set_compact(false);
    let generation = s.reload();
    s.publish(generation, Vec::new(), false, false);
    assert_eq!(s.control, Control::Catalog);
    assert_eq!(s.empty_action(), Some(EmptyAction::PlaylistSettings(2)));
    // Sent off again, then Search closed while a read is pending: the
    // reply shows the action, and the remote waits on Search rather
    // than landing there and being sent off.
    s.set_library(VodState::Empty { source: 3 });
    assert_eq!(s.control, Control::Category);
    s.control = Control::Search;
    s.open_search();
    let generation = s.reload();
    s.close_search();
    s.publish(generation, Vec::new(), false, false);
    assert_eq!(s.empty_action(), Some(EmptyAction::Refresh(3)));
    assert_eq!(s.control, Control::Search);
    // Down from there enters it on purpose.
    s.browse_vertical(1);
    assert_eq!(s.control, Control::Catalog);
    s.set_compact(true);
    assert_eq!(s.control, Control::Catalog);
}
#[test]
fn the_catalog_keeps_its_toolbar() {
    let mut s = Screen::default();
    let generation = s.begin(Kind::Series);
    assert!(!s.is_shelf());
    assert_eq!(
        s.controls(),
        vec![
            Control::Search,
            Control::Source,
            Control::Category,
            Control::Sort,
            Control::Continue,
        ]
    );
    s.publish(
        generation,
        vec![keyed(1, Kind::Series, "generated")],
        false,
        false,
    );
    assert!(s.controls().contains(&Control::Catalog));
}
#[test]
fn series_resume_keeps_the_last_watched_episode_and_advances_after_completion() {
    let mut item = card("generated-series");
    item.key.kind = Kind::Series;
    let episodes: Vec<_> = (1..=6)
        .map(|number| Episode {
            key: Key {
                source: 1,
                kind: Kind::Episode,
                id: format!("episode-{number}"),
            },
            season: if number < 5 { 1 } else { 2 },
            number,
            title: format!("Generated episode {number}"),
            plot: None,
            duration_secs: Some(120),
            progress: Progress::default(),
        })
        .collect();
    let fifth = episodes[4].key.clone();
    let sixth = episodes[5].key.clone();
    let mut details = Details {
        incarnation: "generated".into(),
        item,
        episodes,
        resume_episode: Some(fifth.clone()),
    };
    details.episodes[4].progress = Progress::observed(37_000, Some(120_000), false);
    let mut screen = Screen::default();
    screen.show_details(details.clone());
    assert_eq!(screen.episode, Some(fifth.clone()));
    assert_eq!(screen.season, Some(2));
    details.episodes[4].progress.completed = true;
    screen.show_details(details.clone());
    assert_eq!(screen.episode, Some(sixth.clone()));
    details.resume_episode = Some(sixth.clone());
    details.episodes[5].progress.completed = true;
    screen.show_details(details.clone());
    assert_eq!(
        screen.episode,
        Some(sixth),
        "a completed tail never jumps back to unrelated episode one"
    );
    details.resume_episode = None;
    screen.show_details(details.clone());
    assert_eq!(screen.episode, Some(details.episodes[0].key.clone()));
}
#[test]
fn back_from_a_played_episode_reopens_its_show_on_that_episode() {
    let mut s = Screen::default();
    let g = s.begin(Kind::Series);
    let mut details = series_details(4);
    details.episodes[2].season = 2;
    details.episodes[3].season = 2;
    s.publish(g, vec![details.item.clone()], false, false);
    s.show_details(details.clone());
    let played = details.episodes[3].key.clone();
    s.hand_to_player();
    assert!(!s.open && s.returns_to(&details.item.key));
    assert!(!s.returns_to(&card("other").key));
    s.return_from_player(details.clone(), &played, true);
    assert!(s.open && !s.returns_to(&details.item.key));
    assert_eq!(s.episode, Some(played));
    assert_eq!(s.season, Some(2));
    assert_eq!(s.control, Control::Play);
    assert_eq!(s.handoff, Handoff::Returned);
    // A second Back leaves the details for the list, on the same title.
    s.back();
    assert!(s.open && s.details.is_none());
    assert_eq!(s.focused, Some(details.item.key.clone()));
    assert_eq!(s.handoff, Handoff::None);
}
#[test]
fn back_from_a_played_episode_shows_its_series_progress_on_the_list() {
    let mut s = Screen::default();
    let g = s.begin(Kind::Series);
    let mut details = series_details(3);
    s.publish(
        g,
        vec![card("a"), details.item.clone(), card("b")],
        false,
        false,
    );
    s.show_details(details.clone());
    let first = details.episodes[0].key.clone();
    // Part of the first episode watched: the card resumes it.
    s.hand_to_player();
    details.episodes[0].progress = Progress::observed(120_000, Some(600_000), false);
    s.return_from_player(details.clone(), &first, true);
    assert_eq!(s.cards[1].progress, details.episodes[0].progress);
    assert_eq!(
        s.details.as_ref().map(|d| d.item.progress),
        Some(s.cards[1].progress)
    );
    s.back();
    assert_eq!(s.focused, Some(details.item.key.clone()));
    assert_eq!(s.cards[1].progress.resume_ms(), 120_000);
    // Finished: no resume point, the series not yet watched.
    s.show_details(details.clone());
    s.hand_to_player();
    details.episodes[0].progress = Progress::observed(600_000, Some(600_000), true);
    s.return_from_player(details.clone(), &first, true);
    assert_eq!(s.cards[1].progress, Progress::default());
    // Its last episode finished: the series is watched.
    let last = details.episodes[2].key.clone();
    s.hand_to_player();
    details.episodes[2].progress = Progress::observed(600_000, Some(600_000), true);
    s.return_from_player(details.clone(), &last, true);
    assert!(s.cards[1].progress.completed);
    assert_eq!(s.cards[1].progress.resume_ms(), 0);
    // Another episode left before the player saved anything: the
    // store still describes the last one, and so does the card.
    let watched = s.cards[1].progress;
    s.hand_to_player();
    s.return_from_player(details.clone(), &details.episodes[1].key.clone(), false);
    assert_eq!(s.cards[1].progress, watched);
    // The other cards are untouched.
    assert_eq!(s.cards[0].progress, Progress::default());
    assert_eq!(s.cards[2].progress, Progress::default());
}
#[test]
fn back_from_a_played_movie_shows_its_progress_on_the_list() {
    let mut s = Screen::default();
    let g = s.begin(Kind::Movie);
    s.publish(g, vec![card("a"), card("b")], false, false);
    let mut details = Details {
        incarnation: "generated".into(),
        item: card("b"),
        episodes: vec![],
        resume_episode: None,
    };
    s.show_details(details.clone());
    s.hand_to_player();
    details.item.progress = Progress::observed(120_000, Some(600_000), false);
    s.return_from_player(details.clone(), &details.item.key.clone(), true);
    assert_eq!(s.control, Control::Play);
    assert_eq!(s.focused, Some(card("b").key));
    assert_eq!(s.cards[1].progress, details.item.progress);
    assert!(s.resumes());
    // Choosing anything else ends the handoff.
    s.hand_to_player();
    s.begin(Kind::Series);
    assert!(!s.returns_to(&card("b").key));
}

#[test]
fn the_rail_leaves_out_destinations_that_can_never_show_anything() {
    let mut s = Screen::default();
    assert_eq!(
        s.destinations(),
        [
            Destination::Live,
            Destination::Movies,
            Destination::Shows,
            Destination::Recordings
        ]
    );
    // Every enabled playlist is M3U, and the host never records.
    s.set_navigation(false, false);
    s.begin(Kind::Movie);
    assert_eq!(s.destinations(), [Destination::Live], "kept across begin");
    s.browse_horizontal(-1);
    assert_eq!(s.control, Control::Navigation);
    assert_eq!(s.destination, Destination::Live, "not the hidden Movies");
    s.browse_vertical(1);
    assert_eq!(s.destination, Destination::Live);
    // An Xtream playlist arrives: Movies and Shows come back.
    s.set_navigation(true, false);
    s.browse_vertical(1);
    assert_eq!(s.destination, Destination::Movies);
    s.browse_vertical(5);
    assert_eq!(s.destination, Destination::Shows, "no Recordings");
    // And go again with the remote on them.
    s.set_navigation(false, true);
    assert_eq!(s.destination, Destination::Live);
    s.browse_vertical(1);
    assert_eq!(s.destination, Destination::Recordings);
}

#[test]
fn closing_the_catalog_discards_the_player_handoff_and_stale_replies() {
    let mut screen = Screen::default();
    screen.begin(Kind::Series);
    let details = series_details(2);
    let key = details.item.key.clone();
    screen.show_details(details);
    screen.hand_to_player();
    assert!(screen.returns_to(&key));
    let generation = screen.generation;
    screen.close();
    assert!(!screen.open);
    assert!(!screen.returns_to(&key));
    assert_ne!(screen.generation, generation);
    assert!(!screen.publish(generation, vec![card("late")], false, false));
}

#[test]
fn losing_title_authorization_clears_the_details_and_episode_by_identity() {
    let mut screen = Screen::default();
    screen.begin(Kind::Series);
    let details = series_details(2);
    let key = details.item.key.clone();
    screen.show_details(details);
    screen.close_blocked_details(&card("other").key);
    assert_eq!(screen.control, Control::Play);
    assert!(screen.episode.is_some());

    screen.close_blocked_details(&key);
    assert!(screen.open);
    assert_eq!(screen.focused, Some(key));
    assert!(screen.details.is_none());
    assert!(screen.episode.is_none());
    assert_eq!(screen.control, Control::Catalog);
}

#[test]
fn catalog_filters_and_title_page_belong_to_the_same_read() {
    let mut screen = Screen::default();
    let generation = screen.begin_sorted(Kind::Movie, Sort::Rating);
    assert_eq!(screen.query.sort, Sort::Rating);
    let page = DiscoveryPage {
        cards: vec![card("generated")],
        more: false,
        start: 0,
    };
    let source = Source {
        id: 1,
        name: "Generated playlist".into(),
    };
    assert!(!screen.publish_catalog(
        generation.wrapping_sub(1),
        page.clone(),
        vec![source.clone()],
        vec![],
        false
    ));
    assert!(screen.sources.is_empty());
    assert!(screen.publish_catalog(generation, page, vec![source], vec![], false));
    assert_eq!(screen.cards.len(), 1);
    screen.choose_catalog_group(CatalogGroup::ContinueWatching);
    assert_eq!(screen.control, Control::Category);
    assert!(screen.query.continue_watching);
    screen.choose_catalog_group(CatalogGroup::Category {
        source: 1,
        id: "generated".into(),
    });
    assert_eq!(screen.query.category, Some((1, "generated".into())));
    assert!(!screen.query.continue_watching);
    screen.cycle_source();
    assert_eq!(screen.query.source, Some(1));
    assert!(screen.query.category.is_none());
}

#[test]
fn back_refreshes_changed_progress_without_forgetting_a_deep_catalog_page() {
    let mut screen = Screen::default();
    let generation = screen.begin(Kind::Series);
    let details = series_details(2);
    let key = details.item.key.clone();
    screen.publish(generation, vec![details.item.clone()], true, false);
    screen.choose_catalog_group(CatalogGroup::ContinueWatching);
    screen.show_details(details.clone());
    screen.hand_to_player();
    screen.return_from_player(details, &key, false);
    assert_eq!(screen.back(), BackOutcome::Reload { through: 1 });
    assert_eq!(screen.control, Control::Catalog);
    assert_eq!(screen.focused, Some(key));
}

#[test]
fn the_next_episode_can_be_offered_in_existing_or_reopened_details() {
    let mut screen = Screen::default();
    screen.begin(Kind::Series);
    let details = series_details(2);
    let key = details.episodes[1].key.clone();
    let season = details.episodes[1].season;
    screen.show_details(details.clone());
    screen.select_episode(key.clone());
    assert_eq!(screen.control, Control::Episodes);
    screen.select_next_episode(details.clone(), key.clone(), season, None);
    assert_eq!(screen.control, Control::Episodes);
    screen.hand_to_player();
    screen.reopen_for_next_episode(details, key.clone(), season, "Ready");
    assert!(screen.open && !screen.busy);
    assert_eq!(screen.control, Control::Play);
    assert_eq!(screen.episode, Some(key));
    assert_eq!(screen.season, Some(season));
    assert_eq!(screen.notice.as_deref(), Some("Ready"));
}

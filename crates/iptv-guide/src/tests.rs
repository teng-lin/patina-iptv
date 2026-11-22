use iptv_core::guide::{Airing, Program, Schedule, now_minute};
use iptv_epg::{
    ChannelKey, FeedId, GuideStore, MatchIndex, Overrides, ParseOptions, PlaylistChannel,
    SourceBuilder, StoreOptions, format_xmltv_time, parse_xmltv,
};
use iptv_providers::m3u::parse_m3u;

use super::{EpgSchedule, GuideChannel, NO_INFORMATION};

/// Hour-aligned "now" for readable minutes.
pub(crate) const NOW: i64 = 1_800_000_000;

/// Channel `a.test` has programs around now with a gap; `b.test` has none.
fn xmltv() -> String {
    let p = |from: i64, to: i64, title: &str| {
        format!(
            "<programme channel=\"a.test\" start=\"{}\" stop=\"{}\"><title>{title}</title><desc>About {title}.</desc></programme>",
            format_xmltv_time(NOW + from * 60),
            format_xmltv_time(NOW + to * 60),
        )
    };
    format!(
        "<?xml version=\"1.0\"?><tv><channel id=\"a.test\"><display-name>Alpha</display-name></channel>{}{}{}</tv>",
        p(-30, 30, "Alpha News"),
        p(30, 60, "Alpha Talk"),
        p(90, 120, "Alpha Late"),
    )
}

fn store_for(keys: &[(&ChannelKey, Option<&str>, &str)]) -> GuideStore {
    store_with(keys, true)
}

fn store_with(keys: &[(&ChannelKey, Option<&str>, &str)], descriptions: bool) -> GuideStore {
    store_from(&xmltv(), keys, descriptions)
}

fn store_from(
    xml: &str,
    keys: &[(&ChannelKey, Option<&str>, &str)],
    descriptions: bool,
) -> GuideStore {
    let parsed =
        parse_xmltv(xml.as_bytes(), ParseOptions::default()).expect("synthetic xmltv parses");
    let mut builder = SourceBuilder::new(
        FeedId(1),
        0,
        StoreOptions {
            store_descriptions: descriptions,
            ..Default::default()
        },
        NOW,
    );
    parsed
        .channels
        .into_iter()
        .for_each(|c| builder.push_channel(c));
    parsed
        .programs
        .into_iter()
        .for_each(|p| builder.push_program(p));
    let source = builder.finish();
    let mut index = MatchIndex::new();
    index.add_source(source.id(), source.priority(), source.channels());
    let matched = index.match_channels(
        keys.iter().map(|(key, tvg_id, name)| PlaylistChannel {
            key,
            tvg_id: *tvg_id,
            name,
            tvg_name: None,
        }),
        &Overrides::default(),
    );
    let mut store = GuideStore::new();
    store.replace_source(source);
    store.set_matches(matched.matches);
    store
}

pub(crate) fn schedule() -> EpgSchedule {
    let (a, b) = (ChannelKey::new("a"), ChannelKey::new("b"));
    let store = store_for(&[(&a, Some("a.test"), "Alpha"), (&b, None, "Beta")]);
    let ch = |key: ChannelKey, name: &str, days| GuideChannel {
        key,
        name: name.to_owned(),
        number: 0,
        catchup_days: days,
    };
    EpgSchedule::new(
        vec![("News".to_owned(), vec![ch(a, "Alpha", 3), ch(b, "Beta", 0)])],
        store,
        NOW,
    )
}

pub(crate) fn mid_minute_schedule() -> EpgSchedule {
    // "Second" starts 30 s into minute 30: the minute rounds down to it.
    let p = |from: i64, to: i64, title: &str| {
        format!(
            "<programme channel=\"a.test\" start=\"{}\" stop=\"{}\"><title>{title}</title></programme>",
            format_xmltv_time(NOW + from),
            format_xmltv_time(NOW + to),
        )
    };
    let xml = format!(
        "<?xml version=\"1.0\"?><tv><channel id=\"a.test\"><display-name>Alpha</display-name></channel>{}{}</tv>",
        p(0, 30 * 60 + 30, "First"),
        p(30 * 60 + 30, 60 * 60, "Second"),
    );
    let a = ChannelKey::new("a");
    let store = store_from(&xml, &[(&a, Some("a.test"), "Alpha")], false);
    let channel = GuideChannel {
        key: a,
        name: "Alpha".to_owned(),
        number: 0,
        catchup_days: 0,
    };
    EpgSchedule::new(vec![("News".to_owned(), vec![channel])], store, NOW)
}

#[test]
fn a_program_starting_mid_minute_owns_that_minute() {
    let s = mid_minute_schedule();
    let second = s.program_at(0, 30);
    assert_eq!(s.title(0, second), "Second", "the minute it starts in");
    assert_eq!(second, Program { start: 30, end: 60 });
    let first = s.program_at(0, 29);
    assert_eq!(s.title(0, first), "First");
    assert_eq!(first.end, 30, "cells meet without overlapping");
    let cells = s.programs_between(0, 0, 60);
    assert_eq!(cells, vec![first, second]);
}

#[test]
fn real_programs_map_to_guide_minutes_with_titles_and_descriptions() {
    let s = schedule();
    let p = s.program_at(0, 0);
    assert_eq!(
        p,
        Program {
            start: -30,
            end: 30
        }
    );
    assert_eq!(s.title(0, p), "Alpha News");
    assert_eq!(s.description(0, p).as_deref(), Some("About Alpha News."));
    assert_eq!(s.catchup_days(0), 3);
}

#[test]
fn descriptions_left_out_of_memory_come_from_the_lookup() {
    let a = ChannelKey::new("a");
    let store = store_with(&[(&a, Some("a.test"), "Alpha")], false);
    let ch = GuideChannel {
        key: a,
        name: "Alpha".to_owned(),
        number: 1,
        catchup_days: 0,
    };
    let plain = EpgSchedule::new(vec![("News".to_owned(), vec![ch.clone()])], store, NOW);
    let p = plain.program_at(0, 0);
    assert_eq!(plain.description(0, p), None);
    let store = store_with(&[(&ch.key, Some("a.test"), "Alpha")], false);
    let lazy = EpgSchedule::new(vec![("News".to_owned(), vec![ch])], store, NOW)
        .with_description_lookup(|key, start| Some(format!("{key} at {}", start - NOW)));
    assert_eq!(lazy.description(0, p).as_deref(), Some("a at -1800"));
    // No program there: nothing to look up.
    assert_eq!(lazy.description(0, lazy.program_at(0, 70)), None);
}

#[test]
fn gaps_are_filled_with_slots_clipped_to_neighbors() {
    let s = schedule();
    let gap = s.program_at(0, 70);
    assert_eq!(
        gap,
        Program { start: 60, end: 90 },
        "between Alpha Talk and Alpha Late"
    );
    assert_eq!(s.title(0, gap), NO_INFORMATION);
    assert_eq!(
        s.program_at(1, 10),
        Program { start: 0, end: 60 },
        "no EPG: hour slots"
    );
}

#[test]
fn programs_in_a_window_are_contiguous() {
    let s = schedule();
    for ch in 0..2 {
        let ps = s.programs_between(ch, -30, 150);
        assert!(
            ps.first().unwrap().start <= -30 && ps.last().unwrap().end >= 120,
            "{ps:?}"
        );
        for w in ps.windows(2) {
            assert_eq!(w[0].end, w[1].start, "channel {ch}: {ps:?}");
        }
    }
    let alpha = s.programs_between(0, -30, 150);
    assert_eq!(
        alpha,
        [
            Program {
                start: -30,
                end: 30
            },
            Program { start: 30, end: 60 },
            Program { start: 60, end: 90 },
            Program {
                start: 90,
                end: 120
            },
        ]
    );
}

#[test]
fn a_catalog_becomes_grouped_channels() {
    let m3u = parse_m3u(
        "#EXTM3U\n\
         #EXTINF:-1 tvg-id=\"a.test\" group-title=\"News\" catchup=\"default\" catchup-days=\"2\" catchup-source=\"http://archive.example.com/a?utc={utc}\",Alpha\n\
         http://live.example.com/a.m3u8\n\
         #EXTINF:-1 group-title=\"Kids\" tvg-logo=\"http://logos.example.com/k.png\",Kappa\n\
         http://live.example.com/k.m3u8\n\
         #EXTINF:-1 group-title=\"News\",Beta\n\
         http://live.example.com/b.m3u8\n\
         #EXTINF:-1,Loose\n\
         http://live.example.com/l.m3u8\n",
    );
    let s = EpgSchedule::from_catalog(&m3u.groups, &m3u.channels.items, GuideStore::new(), NOW);
    let names: Vec<String> = (0..s.group_count()).map(|g| s.group_name(g)).collect();
    assert_eq!(names, ["News", "Kids", "Other"]);
    assert_eq!(s.group_start(1), 2);
    let channels: Vec<String> = (0..s.channel_count()).map(|c| s.channel_name(c)).collect();
    assert_eq!(channels, ["Alpha", "Beta", "Kappa", "Loose"]);
    assert_eq!(s.catchup_days(0), 2);
    assert_eq!(s.catchup_days(1), 0);
    // "All channels" lists them as the playlist does.
    assert_eq!(s.playlist_order(), [0, 2, 1, 3]);
    assert_eq!(
        s.channel_logo(2).as_deref(),
        Some("http://logos.example.com/k.png")
    );
    assert_eq!(s.channel_logo(0), None);
}

#[test]
fn the_concrete_schedule_is_reachable_behind_the_trait() {
    let s: std::rc::Rc<dyn Schedule> = std::rc::Rc::new(schedule());
    let epg = s.as_any().and_then(|a| a.downcast_ref::<EpgSchedule>());
    assert!(epg.is_some_and(|e| e.channel(0).is_some()));
    let synthetic: &dyn Schedule = &iptv_core::guide::SyntheticSchedule;
    assert!(synthetic.as_any().is_none());
}

#[test]
fn the_epoch_is_the_import_hour_and_now_follows_the_wall_clock() {
    let (a, b) = (ChannelKey::new("a"), ChannelKey::new("b"));
    let store = store_for(&[(&a, Some("a.test"), "Alpha"), (&b, None, "Beta")]);
    let ch = |key, number| GuideChannel {
        key,
        name: String::new(),
        number,
        catchup_days: 0,
    };
    // Imported 25 minutes and 13 seconds past the hour.
    let s = EpgSchedule::new(
        vec![("News".to_owned(), vec![ch(a, 7), ch(b, 8)])],
        store,
        NOW + 25 * 60 + 13,
    );
    assert_eq!((s.epoch(), s.epoch_unix()), (NOW, NOW));
    assert!(s.follows_wall_clock());
    assert_eq!((s.minute(NOW + 90), s.unix(-30)), (1, NOW - 1800));
    // Programs keep their minutes from the epoch, whatever the time now.
    let news = Program {
        start: -30,
        end: 30,
    };
    assert_eq!(s.program_at(0, 0), news);
    let now = now_minute(&s, NOW + 25 * 60 + 13, 0);
    assert_eq!(now, 25);
    assert_eq!(news.airing(now), Airing::Live);
    // Five minutes later the next program is live without a rebuild.
    let later = now_minute(&s, NOW + 30 * 60, 0);
    assert_eq!(news.airing(later), Airing::Ended);
    assert_eq!((s.channel_number(0), s.channel_number(1)), (7, 8));
}

#[test]
fn catchup_urls_carry_epoch_times_and_numbers_come_from_the_playlist() {
    let m3u = parse_m3u(
        "#EXTM3U\n\
         #EXTINF:-1 group-title=\"Kids\",Kappa\n\
         http://live.example.com/k.m3u8\n\
         #EXTINF:-1 tvg-chno=\"42\" group-title=\"News\" catchup=\"default\" catchup-days=\"2\" catchup-source=\"http://archive.example.com/a?utc={utc}&end={utcend}&now={lutc}\",Alpha\n\
         http://live.example.com/a.m3u8\n\
         #EXTINF:-1 group-title=\"News\",Beta\n\
         http://live.example.com/b.m3u8\n",
    );
    let s = EpgSchedule::from_catalog(
        &m3u.groups,
        &m3u.channels.items,
        GuideStore::new(),
        NOW + 600,
    );
    let names: Vec<String> = (0..s.channel_count()).map(|c| s.channel_name(c)).collect();
    assert_eq!(names, ["Kappa", "Alpha", "Beta"]);
    let numbers: Vec<u32> = (0..s.channel_count())
        .map(|c| s.channel_number(c))
        .collect();
    assert_eq!(numbers, [1, 42, 3], "tvg-chno, else the playlist position");
    let past = Program {
        start: -120,
        end: -60,
    };
    let url = s.catchup_url(1, past, 15).expect("archived program");
    assert_eq!(
        url,
        format!(
            "http://archive.example.com/a?utc={}&end={}&now={}",
            NOW - 7200,
            NOW - 3600,
            NOW + 900
        )
    );
    // A seek 10 minutes in opens the archive from there to the same end.
    let sought = s
        .resolve_from(1, Some(past), 600, s.clock_at(15))
        .ready()
        .expect("archived program");
    assert_eq!(
        sought.url,
        format!(
            "http://archive.example.com/a?utc={}&end={}&now={}",
            NOW - 7200 + 600,
            NOW - 3600,
            NOW + 900
        )
    );
}

#[test]
fn titles_stored_with_superscript_badges_show_them_as_words() {
    // A guide stored before import folded badges: the title still carries
    // "ᴸᶦᵛᵉ", which the guide, header, info card and play bar show as text.
    let mut parsed = parse_xmltv(xmltv().as_bytes(), ParseOptions::default()).expect("parses");
    parsed.programs[0].title = "Alpha Newsᴸᶦᵛᵉ".to_owned();
    let mut builder = SourceBuilder::new(FeedId(1), 0, StoreOptions::default(), NOW);
    parsed
        .channels
        .into_iter()
        .for_each(|c| builder.push_channel(c));
    parsed
        .programs
        .into_iter()
        .for_each(|p| builder.push_program(p));
    let source = builder.finish();
    let a = ChannelKey::new("a");
    let mut index = MatchIndex::new();
    index.add_source(source.id(), source.priority(), source.channels());
    let matched = index.match_channels(
        [PlaylistChannel {
            key: &a,
            tvg_id: Some("a.test"),
            name: "Alpha",
            tvg_name: None,
        }],
        &Overrides::default(),
    );
    let mut store = GuideStore::new();
    store.replace_source(source);
    store.set_matches(matched.matches);
    let channel = GuideChannel {
        key: a,
        name: "Alpha".to_owned(),
        number: 0,
        catchup_days: 0,
    };
    let s = EpgSchedule::new(vec![("News".to_owned(), vec![channel])], store, NOW);
    assert_eq!(s.title(0, s.program_at(0, 0)), "Alpha News Live");
    assert_eq!(s.title(0, s.program_at(0, 45)), "Alpha Talk");
}

/// A store cannot hold a group id twice, but the constructor must not
/// depend on that: a repeated id is one group, and every group has
/// exactly one key, one name and one first channel.
#[test]
fn a_repeated_group_id_keeps_keys_names_and_starts_together() {
    let m = parse_m3u(
        "#EXTM3U\n\
         #EXTINF:-1 group-title=\"News\",A\nhttp://example.test/a\n\
         #EXTINF:-1 group-title=\"Sport\",B\nhttp://example.test/b\n\
         #EXTINF:-1 group-title=\"News\",C\nhttp://example.test/c\n\
         #EXTINF:-1,D\nhttp://example.test/d\n",
    );
    let (news, sport) = (m.groups[0].clone(), m.groups[1].clone());
    let mut again = news.clone();
    again.name = "News again".to_owned();
    let repeated = [news.clone(), again, sport.clone(), sport.clone()];
    let s = EpgSchedule::from_catalog(&repeated, &m.channels.items, GuideStore::new(), NOW);
    let plain = EpgSchedule::from_catalog(&m.groups, &m.channels.items, GuideStore::new(), NOW);

    let groups = |s: &EpgSchedule| {
        (0..s.group_count())
            .map(|g| (s.group_key(g), s.group_name(g), s.group_channels(g)))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        groups(&s),
        [
            (news.id.as_str().to_owned(), "News".to_owned(), vec![0, 1]),
            (sport.id.as_str().to_owned(), "Sport".to_owned(), vec![2]),
            ("__other__".to_owned(), "Other".to_owned(), vec![3]),
        ]
    );
    assert_eq!(groups(&s), groups(&plain), "the repeats add nothing");
    assert_eq!(
        (
            s.group_keys.len(),
            s.group_names.len(),
            s.group_starts.len()
        ),
        (3, 3, 3)
    );

    // Arranging the groups walks every key and reads its name: with more
    // keys than names this indexed past the end.
    s.set_organization(super::Organization {
        channels: Vec::new(),
        groups: vec![(
            sport.id.as_str().to_owned(),
            "Sport first".to_owned(),
            false,
        )],
        ..Default::default()
    });
    assert_eq!(
        (0..s.group_count())
            .map(|g| s.group_name(g))
            .collect::<Vec<_>>(),
        ["Sport first", "News", "Other"]
    );
}

/// A schedule prepared on another thread is the one organized in place.
#[test]
fn a_prepared_schedule_crosses_threads_and_matches_one_organized_in_place() {
    use super::Organization;
    fn send<T: Send>(value: T) -> T {
        value
    }
    let parsed = parse_m3u(
        "#EXTM3U\n\
         #EXTINF:-1 tvg-id=\"a\" group-title=\"News\",Alpha\nhttp://example.test/a\n\
         #EXTINF:-1 tvg-id=\"b\" group-title=\"Sport\",Beta\nhttp://example.test/b\n\
         #EXTINF:-1 tvg-id=\"c\" group-title=\"News\",Gamma\nhttp://example.test/c\n",
    );
    let (groups, channels) = (parsed.groups, parsed.channels.items);
    let id = |n: usize| channels[n].id.as_str().to_owned();
    let organization = Organization {
        channels: vec![
            (id(2), "Renamed".into(), false),
            (id(0), "Alpha".into(), true),
        ],
        groups: vec![(groups[1].id.as_str().to_owned(), "Games".into(), false)],
        ..Default::default()
    };
    let lists = vec![("mine".to_owned(), "Mine".to_owned(), vec![id(2), id(1)])];
    let in_place = EpgSchedule::from_catalog(&groups, &channels, GuideStore::default(), NOW);
    in_place.set_organization(organization.clone());
    in_place.set_group_lists(lists.clone());
    let (g, c) = (groups.clone(), channels.clone());
    let prepared = std::thread::spawn(move || {
        send(EpgSchedule::prepare(
            &g,
            &c,
            GuideStore::default(),
            NOW,
            Some((organization, lists)),
        ))
    })
    .join()
    .unwrap();
    let prepared = EpgSchedule::from_prepared(prepared);
    let view = |s: &EpgSchedule| {
        (
            (0..s.channel_count())
                .map(|c| (s.channel_name(c), s.channel_hidden(c), s.channel_key(c)))
                .collect::<Vec<_>>(),
            (0..s.group_count())
                .map(|g| (s.group_key(g), s.group_name(g), s.group_channels(g)))
                .collect::<Vec<_>>(),
            s.playlist_order(),
        )
    };
    assert_eq!(view(&prepared), view(&in_place));
    assert_eq!(
        prepared.channel_name(prepared.find_channel(&id(2)).unwrap()),
        "Renamed"
    );
    let plain = EpgSchedule::prepare(&groups, &channels, GuideStore::default(), NOW, None);
    let plain = EpgSchedule::from_prepared(plain);
    let fresh = EpgSchedule::from_catalog(&groups, &channels, GuideStore::default(), NOW);
    assert_eq!(view(&plain), view(&fresh));
}

#[test]
fn generated_guide_placeholders_localize_but_provider_titles_do_not() {
    let ja = iptv_core::i18n::resolve("ja", None);
    let s = schedule();
    assert_eq!(s.title_in(1, s.program_at(1, 0), ja), "情報なし");
    assert_eq!(s.title_in(0, s.program_at(0, 0), ja), "Alpha News");
    // The same bytes can be a genuine provider title or an app placeholder.
    let a = ChannelKey::new("a");
    let xml = xmltv().replace("Alpha News", "No information");
    let store = store_from(&xml, &[(&a, Some("a.test"), "Alpha")], true);
    let s = EpgSchedule::new(
        vec![(
            "Other".into(),
            vec![GuideChannel {
                key: a,
                name: "Alpha".into(),
                number: 1,
                catchup_days: 0,
            }],
        )],
        store,
        NOW,
    );
    assert_eq!(s.title_in(0, s.program_at(0, 0), ja), "No information");
    assert_eq!(
        s.group_name_in(0, ja),
        "Other",
        "provider group name stays opaque"
    );
}

#[test]
fn a_provider_group_using_the_reserved_bucket_key_still_keeps_its_name() {
    let mut parsed = parse_m3u(
        "#EXTM3U\n#EXTINF:-1 group-title=\"Other\",Synthetic Channel\nhttps://synthetic.example.test/live\n",
    );
    parsed.groups[0].id = iptv_providers::id::GroupId::from_stored("__other__");
    parsed.channels.items[0].groups = vec![parsed.groups[0].id.clone()];
    let s = EpgSchedule::from_catalog(
        &parsed.groups,
        &parsed.channels.items,
        GuideStore::new(),
        NOW,
    );
    assert_eq!(
        s.group_name_in(0, iptv_core::i18n::resolve("ja", None)),
        "Other"
    );
}

#[test]
fn the_generated_ungrouped_bucket_localizes_after_prepare_and_organization() {
    let parsed = parse_m3u(
        "#EXTM3U\n#EXTINF:-1,Synthetic Ungrouped Channel\nhttps://synthetic.example.test/live\n",
    );
    let s = EpgSchedule::from_catalog(
        &parsed.groups,
        &parsed.channels.items,
        GuideStore::new(),
        NOW,
    );
    let ja = iptv_core::i18n::resolve("ja", None);
    assert_eq!(s.group_name(0), "Other");
    assert_eq!(
        s.group_name_in(0, ja),
        iptv_core::i18n::translate(ja, "Other")
    );
    s.set_organization(super::Organization::default());
    assert_eq!(
        s.group_name_in(0, ja),
        iptv_core::i18n::translate(ja, "Other")
    );
    let s = EpgSchedule::from_prepared(EpgSchedule::prepare(
        &parsed.groups,
        &parsed.channels.items,
        GuideStore::new(),
        NOW,
        None,
    ));
    assert_eq!(
        s.group_name_in(0, ja),
        iptv_core::i18n::translate(ja, "Other")
    );
}

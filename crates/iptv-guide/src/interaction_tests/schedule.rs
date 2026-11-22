use crate::EpgSchedule;
use crate::tests::{NOW, mid_minute_schedule, schedule};
use iptv_core::guide::{Focus, Move, Program, Schedule};
use iptv_epg::GuideStore;
use iptv_providers::m3u::parse_m3u;

#[test]
fn the_focus_model_navigates_real_data_and_gaps() {
    let s = schedule();
    let mut f = Focus::default();
    f.apply(Move::Right, &s);
    assert_eq!(f.program(&s), Program { start: 30, end: 60 });
    f.apply(Move::Right, &s);
    assert_eq!(
        f.program(&s),
        Program { start: 60, end: 90 },
        "into the gap"
    );
    f.apply(Move::Down, &s);
    assert_eq!(f.channel, 1);
    assert_eq!(
        f.program(&s),
        Program {
            start: 60,
            end: 120
        },
        "same time column on the EPG-less channel"
    );
    f.apply(Move::Down, &s);
    assert_eq!(f.channel, 1, "clamped to the last channel");
}

#[test]
fn organization_filters_lists_but_preserves_playback_and_group_identity() {
    use iptv_core::lists::{ALL, ChannelList, FIRST_GROUP, ListId};
    let m = parse_m3u(
        "#EXTM3U\n#EXTINF:-1 group-title=\"News\",A\nhttp://example.test/a\n#EXTINF:-1 group-title=\"Sport\",B\nhttp://example.test/b\n#EXTINF:-1 group-title=\"News\",C\nhttp://example.test/c\n",
    );
    let s = EpgSchedule::from_catalog(&m.groups, &m.channels.items, GuideStore::new(), NOW);
    let key = s.channel_key(0).unwrap();
    let group = ListId::of(FIRST_GROUP, &s);
    let url = s.stream_url(0);
    s.set_organization(crate::Organization {
        channels: [2, 1, 0]
            .into_iter()
            .map(|c| (s.channel_key(c).unwrap(), format!("Renamed {c}"), c == 0))
            .collect(),
        groups: vec![
            (m.groups[1].id.as_str().into(), "Other label".into(), false),
            (m.groups[0].id.as_str().into(), "Other label".into(), false),
        ],
        ..Default::default()
    });
    assert_eq!(s.channel_count(), 3);
    assert_eq!(s.find_channel(&key), Some(0));
    assert_eq!(s.stream_url(0), url);
    assert_eq!(s.channel_name(0), "Renamed 0");
    assert_eq!(ChannelList::build(ALL, &s, &[], &[]).channels(), [2, 1]);
    assert_eq!(
        group.find(&s),
        FIRST_GROUP + 1,
        "duplicate display names do not change identity"
    );
    assert_eq!(
        ChannelList::build(FIRST_GROUP + 1, &s, &[], &[]).channels(),
        [1]
    );
    assert_eq!(
        ChannelList::build(0, &s, &[0, 1], &[]).channels(),
        [1],
        "favorites obey visibility without removing membership"
    );
    assert_eq!(
        iptv_core::playback::zap_target(&s, 0, 1),
        1,
        "hidden playing channel can zap to visible neighbor"
    );
    let org = crate::Organization {
        channels: Vec::new(),
        groups: vec![(m.groups[0].id.as_str().into(), "News".into(), true)],
        ..Default::default()
    };
    s.set_organization(org);
    assert_eq!(s.group_count(), 1);
    assert_eq!(ChannelList::build(ALL, &s, &[], &[]).channels(), [2]);
    s.set_organization(crate::Organization::default());
    assert_eq!(s.group_count(), 2);
    assert_eq!(ChannelList::build(ALL, &s, &[], &[]).channels(), [0, 2, 1]);
}

#[test]
fn returning_and_new_channels_keep_provider_identity_and_fill_missing_preferences() {
    let m = parse_m3u(
        "#EXTM3U\n#EXTINF:-1 group-title=\"News\",A\nhttp://example.test/a\n#EXTINF:-1 group-title=\"Sport\",B\nhttp://example.test/b\n#EXTINF:-1 group-title=\"News\",C\nhttp://example.test/c\n",
    );
    let schedule = EpgSchedule::from_catalog(&m.groups, &m.channels.items, GuideStore::new(), NOW);
    let a = m.channels.items[0].id.as_str().to_owned();
    let b = m.channels.items[1].id.as_str().to_owned();
    let c = m.channels.items[2].id.as_str().to_owned();
    let preferences = crate::Organization {
        channels: vec![
            ("temporarily-absent".into(), "Orphan".into(), true),
            (c.clone(), "Local C".into(), true),
            (a.clone(), "Local A".into(), false),
        ],
        groups: Vec::new(),
        ..Default::default()
    };
    schedule.set_organization(preferences.clone());
    let list = iptv_core::lists::ChannelList::build(iptv_core::lists::ALL, &schedule, &[], &[]);
    assert_eq!(
        list.channels()
            .iter()
            .map(|&c| schedule.channel_name(c))
            .collect::<Vec<_>>(),
        ["Local A", "B"]
    );
    assert_eq!(
        schedule.channel_name(schedule.find_channel(&b).unwrap()),
        "B",
        "missing customization falls back to provider name"
    );
    let added = parse_m3u(
        "#EXTM3U\n\
         #EXTINF:-1 group-title=\"News\",D\n\
         http://example.test/d\n\
         #EXTINF:-1 group-title=\"News\",A\n\
         http://example.test/a\n\
         #EXTINF:-1 group-title=\"Sport\",B\n\
         http://example.test/b\n\
         #EXTINF:-1 group-title=\"News\",C\n\
         http://example.test/c\n",
    );
    let refreshed =
        EpgSchedule::from_catalog(&added.groups, &added.channels.items, GuideStore::new(), NOW);
    refreshed.set_organization(preferences);
    let visible = iptv_core::lists::ChannelList::build(iptv_core::lists::ALL, &refreshed, &[], &[]);
    assert_eq!(
        visible
            .channels()
            .iter()
            .map(|&c| refreshed.channel_name(c))
            .collect::<Vec<_>>(),
        ["Local A", "D", "B"]
    );
    assert!(refreshed.channel_hidden(refreshed.find_channel(&c).unwrap()));
    assert!(
        refreshed
            .stream_url(refreshed.find_channel(&a).unwrap())
            .is_some()
    );
}

#[test]
fn a_tap_uses_the_minute_of_a_cell_starting_mid_minute() {
    let s = mid_minute_schedule();
    let second = s.program_at(0, 30);
    let mut focus = Focus::at(10);
    focus.tap(0, second.start, &s);
    assert_eq!(s.title(0, focus.program(&s)), "Second");
}

#[test]
fn guide_focus_advances_when_the_next_program_goes_live() {
    let s = schedule();
    let mut f = Focus::at(25);
    assert!(f.tick(30, &s));
    assert_eq!(f.program(&s), Program { start: 30, end: 60 });
    assert_eq!(s.title(0, f.program(&s)), "Alpha Talk");
}

#[test]
fn a_list_preserves_generated_text_provenance() {
    let parsed = parse_m3u(
        "#EXTM3U\n#EXTINF:-1,Synthetic Ungrouped Channel\nhttps://synthetic.example.test/live\n",
    );
    let s = EpgSchedule::from_prepared(EpgSchedule::prepare(
        &parsed.groups,
        &parsed.channels.items,
        GuideStore::new(),
        NOW,
        None,
    ));
    let ja = iptv_core::i18n::resolve("ja", None);
    // The app renders guide rows through ListView; that adapter must retain
    // generated text provenance and the requested language too.
    use iptv_core::lists::{ChannelList, FIRST_GROUP, ListView};
    let list = ChannelList::build(FIRST_GROUP, &s, &[], &[]);
    let view = ListView::new(std::rc::Rc::new(s), std::rc::Rc::new(list));
    assert_eq!(
        view.group_name_in(0, ja),
        iptv_core::i18n::translate(ja, "Other")
    );
    assert_eq!(
        view.title_in(0, view.program_at(0, 0), ja),
        iptv_core::i18n::translate(ja, "No information")
    );
}

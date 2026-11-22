//! Synthetic schedules with minute-aligned and partial-minute listings.

use crate::{EpgSchedule, GuideChannel};
use iptv_epg::{
    ChannelKey, FeedId, GuideStore, MatchIndex, Overrides, ParseOptions, PlaylistChannel,
    SourceBuilder, StoreOptions, format_xmltv_time, parse_xmltv,
};

/// Hour-aligned "now" for readable minutes.
pub const NOW: i64 = 1_800_000_000;

/// Channel `a.test` has programs around now with a gap; `b.test` has none.
pub fn xmltv() -> String {
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

pub fn store_for(keys: &[(&ChannelKey, Option<&str>, &str)]) -> GuideStore {
    store_with(keys, true)
}

pub fn store_with(keys: &[(&ChannelKey, Option<&str>, &str)], descriptions: bool) -> GuideStore {
    store_from(&xmltv(), keys, descriptions)
}

pub fn store_from(
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

pub fn schedule() -> EpgSchedule {
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

pub fn mid_minute_schedule() -> EpgSchedule {
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

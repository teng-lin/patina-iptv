//! Sources, catalog replace, provider-order reads.

use crate::common;

use common::*;
use iptv_providers::model::{ContentKind, SourceKind};
use iptv_store_sqlite::{
    ChannelQuery, ChannelScope, ImportErrorKind, SCHEMA_VERSION, SourceId, Store, StoreError,
};

fn clock() -> i64 {
    1_800_000_000
}

fn store() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.set_clock(clock);
    s
}

#[test]
fn fresh_store_is_current() {
    let s = store();
    assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(s.journal_mode().unwrap(), "memory");
    #[cfg(not(target_family = "wasm"))]
    {
        let dir = tempfile::tempdir().unwrap();
        let file = Store::open(dir.path().join("store.db")).unwrap();
        assert_eq!(file.journal_mode().unwrap(), "wal");
        drop(file);
        // Reopening runs no migration and keeps the version.
        let again = Store::open(dir.path().join("store.db")).unwrap();
        assert_eq!(again.schema_version().unwrap(), SCHEMA_VERSION);
    }
    // The browser's memory files journal in memory; persistence is the
    // host's chunk write after each commit.
    #[cfg(target_family = "wasm")]
    {
        let files = iptv_store_sqlite::BrowserFiles::new().unwrap();
        let file = Store::open_browser(&files).unwrap();
        assert_eq!(file.journal_mode().unwrap(), "memory");
        drop(file);
        let again = Store::open_browser(&files).unwrap();
        assert_eq!(again.schema_version().unwrap(), SCHEMA_VERSION);
    }
}

#[test]
fn sources_round_trip_with_secrets_split_out() {
    let mut s = store();
    let sources = [
        xtream_source("Xt"),
        m3u_source("List"),
        stalker_source("Portal"),
    ];
    let ids: Vec<SourceId> = sources
        .iter()
        .map(|src| s.add_source(src).unwrap())
        .collect();
    for (id, src) in ids.iter().zip(&sources) {
        let stored = s.source(*id).unwrap().unwrap();
        assert_eq!(&stored.source, src);
        assert_eq!(stored.created_at, clock());
    }
    let list = s.sources().unwrap();
    assert_eq!(
        list.iter()
            .map(|x| (x.name.as_str(), x.kind))
            .collect::<Vec<_>>(),
        [
            ("Xt", SourceKind::Xtream),
            ("List", SourceKind::M3u),
            ("Portal", SourceKind::Stalker)
        ]
    );
    assert_eq!(list[0].import.last_success_at, None);

    s.rename_source(ids[0], "Renamed").unwrap();
    let mut updated = stalker_source("Portal 2");
    if let iptv_providers::model::SourceConfig::Stalker(c) = &mut updated.config {
        c.serial = None;
    }
    s.update_source(ids[2], &updated).unwrap();
    assert_eq!(s.source(ids[2]).unwrap().unwrap().source, updated);
    assert_eq!(s.source(ids[0]).unwrap().unwrap().source.name, "Renamed");

    assert!(s.delete_source(ids[1]).unwrap());
    assert!(!s.delete_source(ids[1]).unwrap());
    assert!(s.source(ids[1]).unwrap().is_none());
    assert!(matches!(
        s.rename_source(ids[1], "x"),
        Err(StoreError::SourceNotFound(_))
    ));
}

#[test]
fn replace_reads_back_in_provider_order() {
    let mut s = store();
    let src = s.add_source(&xtream_source("Xt")).unwrap();
    let cat = catalog(25, 4, XT_PASSWORD);
    let counts = s
        .replace_catalog(src, &cat.groups, &cat.channels, &cat.vod, &cat.series)
        .unwrap();
    assert_eq!(
        (
            counts.groups,
            counts.channels,
            counts.vod,
            counts.series,
            counts.duplicates
        ),
        (5, 25, 3, 1, 0)
    );

    let all = s.load_channels(src, &ChannelQuery::default()).unwrap();
    assert_eq!(all.len(), 25);
    assert_eq!(
        all.iter().map(|r| r.channel.clone()).collect::<Vec<_>>(),
        cat.channels
    );

    let page = s
        .load_channels(
            src,
            &ChannelQuery {
                offset: 10,
                limit: Some(5),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(page[0].channel.name, "Channel 00010");
    assert_eq!(page.len(), 5);

    let g1 = ChannelQuery {
        scope: ChannelScope::Group(group(1).id),
        ..Default::default()
    };
    let in_g1 = s.load_channels(src, &g1).unwrap();
    assert!(in_g1.iter().all(|r| r.channel.groups == [group(1).id]));
    assert_eq!(s.channel_count(src, &g1).unwrap(), in_g1.len() as u64);
    assert_eq!(s.channel_count(src, &ChannelQuery::default()).unwrap(), 25);

    assert_eq!(
        s.groups(src, Some(ContentKind::Live)).unwrap(),
        cat.groups[..4]
    );
    assert_eq!(s.groups(src, None).unwrap(), cat.groups);
    assert_eq!(s.load_vod(src, None, 0, None).unwrap(), cat.vod);
    assert_eq!(
        s.load_vod(src, Some(&cat.vod[0].groups[0]), 1, Some(1))
            .unwrap(),
        cat.vod[1..2]
    );
    assert_eq!(s.load_series(src, None, 0, None).unwrap(), cat.series);

    let one = s.channel(src, &cat.channels[3].id).unwrap().unwrap();
    assert_eq!(one.channel, cat.channels[3]);
}

#[test]
fn duplicates_are_dropped_and_counted() {
    let mut s = store();
    let src = s.add_source(&xtream_source("Xt")).unwrap();
    let mut cat = catalog(5, 1, XT_PASSWORD);
    let dup = cat.channels[1].clone();
    cat.channels.push(dup);
    cat.groups.push(cat.groups[0].clone());
    let counts = s
        .replace_catalog(src, &cat.groups, &cat.channels, &[], &[])
        .unwrap();
    assert_eq!(
        (counts.channels, counts.groups, counts.duplicates),
        (5, 2, 2)
    );
}

#[test]
fn import_status_tracks_success_and_failure() {
    let mut s = store();
    let src = s.add_source(&xtream_source("Xt")).unwrap();
    replace(&mut s, src, &catalog(4, 2, XT_PASSWORD));
    let ok = s.import_status(src).unwrap();
    assert_eq!(ok.last_success_at, Some(clock()));
    assert_eq!(ok.counts.channels, 4);
    assert_eq!(ok.last_error, None);

    s.record_import_failure(src, ImportErrorKind::Timeout)
        .unwrap();
    let failed = s.import_status(src).unwrap();
    assert_eq!(failed.last_error, Some(ImportErrorKind::Timeout));
    assert_eq!(failed.last_success_at, Some(clock()));
    // A failure keeps the old catalog: it never looks like empty content.
    assert_eq!(s.channel_count(src, &ChannelQuery::default()).unwrap(), 4);

    replace(&mut s, src, &catalog(2, 1, XT_PASSWORD));
    assert_eq!(s.import_status(src).unwrap().last_error, None);
    assert_eq!(
        ImportErrorKind::from(&iptv_providers::ProviderError::AccountRejected),
        ImportErrorKind::AccountRejected
    );
}

#[test]
fn replace_is_per_source_and_needs_a_source() {
    let mut s = store();
    let a = s.add_source(&xtream_source("A")).unwrap();
    let b = s.add_source(&m3u_source("B")).unwrap();
    replace(&mut s, a, &catalog(6, 2, XT_PASSWORD));
    replace(&mut s, b, &catalog(3, 1, XT_PASSWORD));
    replace(&mut s, a, &catalog(2, 1, XT_PASSWORD));
    assert_eq!(s.channel_count(a, &ChannelQuery::default()).unwrap(), 2);
    assert_eq!(s.channel_count(b, &ChannelQuery::default()).unwrap(), 3);
    assert!(matches!(
        s.replace_catalog(SourceId(999), &[], &[], &[], &[]),
        Err(StoreError::SourceNotFound(_))
    ));
    s.delete_source(b).unwrap();
    assert_eq!(s.channel_count(b, &ChannelQuery::default()).unwrap(), 0);
}

#[test]
fn stalker_streams_round_trip() {
    let mut s = store();
    let src = s.add_source(&stalker_source("P")).unwrap();
    let g = group(0);
    let mut ch = xtream_channel(5, "Portal One", &g.id);
    ch.stream = stalker_stream("ffrt http://localhost/ch/5");
    s.replace_catalog(src, &[g], std::slice::from_ref(&ch), &[], &[])
        .unwrap();
    assert_eq!(s.channel(src, &ch.id).unwrap().unwrap().channel, ch);
}

/// The guide keeps finished programs for the longest catch-up archive of
/// the stored channels (#282).
#[test]
fn longest_catchup_archive_spans_every_source() {
    let mut s = store();
    assert_eq!(s.longest_catchup_days(7).unwrap(), 0, "nothing stored");
    let src = s.add_source(&xtream_source("Xt")).unwrap();
    // Every third channel has three days of catch-up, some without a window.
    let mut cat = catalog(12, 2, XT_PASSWORD);
    replace(&mut s, src, &cat);
    assert_eq!(s.longest_catchup_days(7).unwrap(), 7, "the default window");
    assert_eq!(s.longest_catchup_days(2).unwrap(), 3);
    for channel in &mut cat.channels {
        if let Some(catch_up) = &mut channel.catch_up {
            catch_up.days = Some(2);
        }
    }
    replace(&mut s, src, &cat);
    assert_eq!(s.longest_catchup_days(7).unwrap(), 2);
    let other = s.add_source(&xtream_source("Other")).unwrap();
    let mut longer = catalog(12, 2, XT_PASSWORD);
    longer.channels[0].catch_up.as_mut().unwrap().days = Some(5);
    replace(&mut s, other, &longer);
    assert_eq!(s.longest_catchup_days(2).unwrap(), 5);
}

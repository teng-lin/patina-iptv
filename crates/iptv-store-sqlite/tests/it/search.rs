//! Name search with FTS5 and with the LIKE fallback.

use crate::common;

use common::*;
use iptv_providers::model::Channel;
use iptv_store_sqlite::{OpenOptions, SearchBackend, Store};

fn named(names: &[&str]) -> (Vec<iptv_providers::model::Group>, Vec<Channel>) {
    let g = group(0);
    let channels = names
        .iter()
        .enumerate()
        .map(|(i, n)| xtream_channel(i as u64 + 1, n, &g.id))
        .collect();
    (vec![g], channels)
}

fn stores() -> Vec<Store> {
    let fts = Store::open_in_memory().unwrap();
    assert_eq!(fts.search_backend(), SearchBackend::Fts5);
    let like = Store::open_in_memory_with(&OpenOptions {
        prefer_fts: false,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(like.search_backend(), SearchBackend::Like);
    vec![fts, like]
}

fn names(rows: &[iptv_store_sqlite::ChannelRow]) -> Vec<&str> {
    rows.iter().map(|r| r.channel.name.as_str()).collect()
}

#[test]
fn search_finds_every_word_on_both_backends() {
    for mut s in stores() {
        let backend = s.search_backend();
        let src = s.add_source(&xtream_source("Xt")).unwrap();
        let (groups, channels) = named(&["News 24 HD", "Sports News", "Movies HD", "Kids"]);
        s.replace_catalog(src, &groups, &channels, &[], &[])
            .unwrap();

        let hits = s.search_channels(Some(src), "news", 10).unwrap();
        assert_eq!(hits.len(), 2, "{backend:?}");
        assert_eq!(
            names(&s.search_channels(Some(src), "hd news", 10).unwrap()),
            ["News 24 HD"]
        );
        assert_eq!(
            names(&s.search_channels(None, "MOV", 10).unwrap()),
            ["Movies HD"]
        );
        assert!(s.search_channels(Some(src), "  ", 10).unwrap().is_empty());
        assert!(
            s.search_channels(Some(src), "weather", 10)
                .unwrap()
                .is_empty()
        );
        assert_eq!(s.search_channels(Some(src), "hd", 1).unwrap().len(), 1);

        // A refresh replaces the index rows of that source only.
        let (groups, channels) = named(&["Weather"]);
        s.replace_catalog(src, &groups, &channels, &[], &[])
            .unwrap();
        assert!(s.search_channels(Some(src), "news", 10).unwrap().is_empty());
        assert_eq!(
            names(&s.search_channels(Some(src), "weath", 10).unwrap()),
            ["Weather"]
        );
    }
}

#[test]
fn fts_is_diacritics_insensitive() {
    let mut s = Store::open_in_memory().unwrap();
    let src = s.add_source(&xtream_source("Xt")).unwrap();
    let (groups, channels) = named(&["Télé Région"]);
    s.replace_catalog(src, &groups, &channels, &[], &[])
        .unwrap();
    assert_eq!(
        s.search_channels(Some(src), "tele region", 5)
            .unwrap()
            .len(),
        1
    );
}

#[test]
#[cfg_attr(target_family = "wasm", ignore = "needs a second connection")]
fn index_is_rebuilt_when_missing_and_dropped_with_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.db");
    let src;
    {
        let mut s = common::open_with(
            &path,
            &OpenOptions {
                prefer_fts: false,
                ..Default::default()
            },
        );
        src = s.add_source(&xtream_source("Xt")).unwrap();
        let (groups, channels) = named(&["Alpha One", "Beta Two"]);
        s.replace_catalog(src, &groups, &channels, &[], &[])
            .unwrap();
    }
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("DROP TABLE channel_fts").unwrap();
    }
    let mut s = common::open(&path);
    assert_eq!(
        names(&s.search_channels(Some(src), "beta", 5).unwrap()),
        ["Beta Two"]
    );
    s.delete_source(src).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    let left: i64 = conn
        .query_row("SELECT COUNT(*) FROM channel_fts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(left, 0);
}

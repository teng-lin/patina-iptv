//! Actual process death while replacing a synthetic encrypted catalog.
use super::*;
use crate::{ChannelQuery, DatabaseKey};
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use std::{
    path::Path,
    process::Command,
    time::{Duration, Instant},
};
const KEY: [u8; 32] = [0x37; 32];

pub(super) fn boundary(at: &str) {
    if std::env::var("IPTV_TEST_CATALOG_CRASH").as_deref() == Ok(at) {
        std::process::exit(74);
    }
}
fn open(path: &Path) -> Store {
    Store::open_encrypted(path, &DatabaseKey::from_bytes(KEY)).unwrap()
}
fn replace(store: &mut Store, source: SourceId, title: &str) {
    let text = format!("#EXTM3U\n#EXTINF:-1,{title}\nhttps://media.example.test/generated.ts\n");
    let parsed = iptv_providers::m3u::parse_m3u(&text);
    store
        .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
        .unwrap();
}
#[test]
fn catalog_crash_child() {
    let Some(path) = std::env::var_os("IPTV_TEST_CATALOG_PATH") else {
        return;
    };
    let mut store = open(Path::new(&path));
    let source = store.sources().unwrap()[0].id;
    replace(&mut store, source, "Generated replacement");
    panic!("catalog boundary was not reached");
}
#[test]
fn catalog_publication_is_atomic_across_process_death() {
    for (at, expected) in [
        ("staged", "Generated current"),
        ("committed", "Generated replacement"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.sqlite");
        let mut store = open(&path);
        let source = store
            .add_source(&Source {
                name: "Generated fixture".into(),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: SecretUrl::new("https://playlist.example.test/generated.m3u"),
                    user_agent: None,
                    referrer: None,
                }),
            })
            .unwrap();
        replace(&mut store, source, "Generated current");
        drop(store);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "catalog::crash_tests::catalog_crash_child",
                "--nocapture",
            ])
            .env("IPTV_TEST_CATALOG_PATH", &path)
            .env("IPTV_TEST_CATALOG_CRASH", at)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("catalog child did not acknowledge {at}");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(status.code(), Some(74));
        let store = open(&path);
        let rows = store
            .load_channels(source, &ChannelQuery::default())
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, expected);
        assert_eq!(
            store
                .channel_count(source, &ChannelQuery::default())
                .unwrap(),
            1
        );
    }
}

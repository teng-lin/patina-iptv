//! Secrets stay out of Debug output, error messages and the config column.

use crate::common;

use common::*;
use iptv_store_sqlite::{ChannelQuery, Store, StoreError};

const SECRETS: &[&str] = &[
    XT_PASSWORD,
    XT_USER,
    "AB:CD:EF",
    "SERIAL-SYNTH",
    "synthetic-login",
];

fn assert_clean(text: &str) {
    for secret in SECRETS {
        assert!(!text.contains(secret), "{secret:?} leaked in {text}");
    }
}

#[test]
fn debug_of_sources_and_rows_redacts() {
    let mut s = Store::open_in_memory().unwrap();
    for source in [
        xtream_source("Xt"),
        m3u_source("List"),
        stalker_source("Portal"),
    ] {
        let id = s.add_source(&source).unwrap();
        let stored = s.source(id).unwrap().unwrap();
        assert_clean(&format!("{stored:?}"));
        assert_clean(&format!("{:?}", s.sources().unwrap()));
        replace(&mut s, id, &catalog(3, 1, XT_PASSWORD));
        let rows = s.load_channels(id, &ChannelQuery::default()).unwrap();
        // The stream URL embeds credentials; the row's Debug leaves it out.
        assert_clean(&format!("{rows:?}"));
        assert_clean(&format!("{s:?}"));
    }
}

#[test]
#[cfg_attr(target_family = "wasm", ignore = "needs a second connection")]
fn secrets_live_only_in_the_credentials_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.db");
    let mut s = common::open(&path);
    for source in [
        xtream_source("Xt"),
        m3u_source("List"),
        stalker_source("Portal"),
    ] {
        s.add_source(&source).unwrap();
    }
    let raw = rusqlite::Connection::open(&path).unwrap();
    let configs: Vec<String> = raw
        .prepare("SELECT config || name FROM sources")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(configs.len(), 3);
    for config in &configs {
        assert_clean(config);
    }
    let secrets: i64 = raw
        .query_row("SELECT COUNT(*) FROM source_credentials", [], |r| r.get(0))
        .unwrap();
    // xtream: username + password; m3u: location; stalker: mac, serial, login, password.
    assert_eq!(secrets, 7);
}

#[test]
#[cfg_attr(target_family = "wasm", ignore = "needs a second connection")]
fn decode_errors_name_the_column_not_the_value() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.db");
    let mut s = common::open(&path);
    let src = s.add_source(&xtream_source("Xt")).unwrap();
    replace(&mut s, src, &catalog(2, 1, XT_PASSWORD));
    let raw = rusqlite::Connection::open(&path).unwrap();
    let leak = format!("http://{XT_USER}:{XT_PASSWORD}@tv.example.net/1.ts");
    for bad in [
        format!(r#"{{"t":"url","url":5,"x":"{leak}"}}"#),
        format!(r#"{{"t":"url","url":"{leak}""#),
        format!(r#"["{leak}"]"#),
    ] {
        raw.execute("UPDATE channels SET stream = ?1", [&bad])
            .unwrap();
        let err = s.load_channels(src, &ChannelQuery::default()).unwrap_err();
        assert!(matches!(err, StoreError::InvalidData(_)), "{err:?}");
        assert_clean(&format!("{err} {err:?}"));
    }
    raw.execute("DELETE FROM source_credentials WHERE name = 'password'", [])
        .unwrap();
    let err = s.source(src).unwrap_err();
    assert_eq!(
        err.to_string(),
        "stored data is invalid: missing credential 'password'"
    );
}

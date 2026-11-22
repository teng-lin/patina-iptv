//! The browser TV package's feature set (hosts/tizen-nsh builds iptv-app
//! without its default features): the screens it leaves out have no way
//! in. Without them the app still imports a playlist and shows its guide,
//! and every place that led to one of them offers nothing in its stead.
//!
//! Each check holds in both builds: with a feature on, its way in is
//! there. The TV build:
//! `cargo test -p iptv-app --no-default-features --features test-support --test app tv_feature_set`
//! (CI's `app-tv-feature-set`). Synthetic data only: a playlist picked
//! from a file.

use nsh_core::{Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;

use crate::support::{self, T};

const PLAYLIST: &str = "#EXTM3U\n\
    #EXTINF:-1 tvg-id=\"one\" group-title=\"News\",Test Channel One\nhttp://stream.example.test/1.m3u8\n\
    #EXTINF:-1 tvg-id=\"two\" group-title=\"News\",Test Channel Two\nhttp://stream.example.test/2.m3u8\n";

#[test]
fn the_tv_build_has_no_way_into_the_screens_it_leaves_out() {
    let Some(_home) = isolated!() else { return };
    let config = EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    let mut t = T::new(config);
    t.app.on_pick(|_| support::picked(PLAYLIST.to_owned()));
    t.wait("text:No playlists yet", 5);
    t.press("text:Add playlist");
    t.wait("text:Playlist file", 5);
    // Without Stalker portals, the type list does not offer them.
    t.wait("text:Xtream Codes login", 5);
    assert_eq!(
        t.shows("text:Stalker portal"),
        cfg!(feature = "stalker"),
        "the setup wizard's Stalker portal"
    );
    t.press("text:Playlist file");
    t.wait("text:Choose a file…", 5);
    t.press("text:Choose a file…");
    t.wait("text:synthetic.m3u", 10);
    t.press("text:Add playlist");
    t.wait("text:Playlist added", 30);
    t.press("text:Watch");
    t.wait("text:Test Channel One", 30);

    menu_and_settings(&mut t);
}

/// Without the guide tools, the channel menu has no "Choose guide
/// data…" and the guide sources screen no "Inspect guide coverage";
/// without Stalker portals, the playlists screen does not name them.
fn menu_and_settings(t: &mut T) {
    t.key("m");
    t.wait("text:Hide channel", 5);
    assert_eq!(
        t.shows("text:Choose guide data…"),
        cfg!(feature = "guide-tools"),
        "the channel menu's guide data chooser"
    );
    t.key("backspace");
    t.gone("text:Hide channel", 5);

    t.press("text:☰  All channels");
    t.key("left");
    t.key("left");
    t.press("text:Playlists >> nth=0");
    t.wait("text:+ Add playlist", 5);
    assert_eq!(
        t.shows("text:M3U link or file, Xtream Codes login, Stalker portal"),
        cfg!(feature = "stalker"),
        "the playlists screen's Stalker portal"
    );
    t.press("text:synthetic");
    t.press("text:Guide sources");
    t.wait("text:Guide sources · synthetic", 5);
    t.wait("text:Update the guide now", 5);
    assert_eq!(
        t.shows("text:Inspect guide coverage"),
        cfg!(feature = "guide-tools"),
        "the guide sources' inspection"
    );
    for _ in 0..3 {
        t.key("backspace");
    }
}

//! "Choose guide data…" (the guide's menu), headless: without stored
//! guide data it says so; with two guide sources added from Settings (TV
//! guide data → Add a guide source), it lists the guide channels in the
//! sources' order, each with its source and whether a channel uses it,
//! narrows them by the filter and by typing, and saves a choice that the
//! guide shows: a channel no other one uses (downloaded first), one whose
//! programs are stored (at once), no guide data, automatic again. With
//! every source switched off the stored guide goes, and the chooser with
//! it.
//!
//! Synthetic data only: a playlist picked from a file, guides served by a
//! local HTTP server (`cov_sources/`).

use nsh_core::{Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;

use crate::support::{self, Server, T};

const TITLE: &str = "text:Guide data for Test Channel Three";

#[test]
fn choosing_a_channels_guide_data() {
    let Some(_home) = isolated!() else { return };
    let now = support::now();
    let srv = Server::start(&[
        (
            "/guide.xml",
            support::guide(
                now,
                &[
                    ("one", "Guide One", "Quiet Harbour"),
                    ("two", "Guide Two", "Golden Engine"),
                    ("spare", "Spare Channel", "Spare Show"),
                ],
            ),
        ),
        (
            "/alt.xml",
            support::guide(
                now,
                &[
                    ("alt-news", "Alt News", "Alt Bulletin"),
                    ("alt-one", "Alt One", "Alt Morning"),
                ],
            ),
        ),
    ]);
    // The provider's guide is missing: nothing stored at first.
    let playlist = format!(
        "#EXTM3U url-tvg=\"{}\"\n\
         #EXTINF:-1 tvg-id=\"one\" group-title=\"News\",Test Channel One\nhttp://stream.example.test/1.m3u8\n\
         #EXTINF:-1 tvg-id=\"two\" group-title=\"News\",Test Channel Two\nhttp://stream.example.test/2.m3u8\n\
         #EXTINF:-1 group-title=\"News\",Test Channel Three\nhttp://stream.example.test/3.m3u8\n",
        srv.url("/missing.xml")
    );
    let config = EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    let mut t = T::new(config);
    t.app.on_pick(move |_| support::picked(playlist.clone()));
    t.wait("text:No playlists yet", 5);
    t.press("text:Add playlist");
    // The wizard's options come on a later frame (slow on a loaded machine).
    t.wait("text:Playlist file", 5);
    t.press("text:Playlist file");
    t.wait("text:Choose a file…", 5);
    t.press("text:Choose a file…");
    // The picker answers asynchronously; submit only once the chosen file
    // is in the form (as the other file-import scenarios do).
    t.wait("text:synthetic.m3u", 10);
    t.press("text:Add playlist");
    t.wait("text:Playlist added", 30);
    t.press("text:Watch");
    t.wait("text:Test Channel Three", 30);

    // No guide data stored: the chooser does not open.
    t.key("m");
    t.press("text:Choose guide data…");
    t.wait(
        "text:No guide data is stored yet: update the guide first",
        2,
    );
    assert!(!t.shows(TITLE));

    add_sources(&mut t, &srv);
    lists_and_narrows(&mut t);
    choices(&mut t, &_home.data_dir().join("iptv.sqlite"));
    every_source_off(&mut t);
}

/// Settings → TV guide data → "Add a guide source" (the guide sources
/// screen opens on a new link), twice; then "Update the guide now".
fn add_sources(t: &mut T, srv: &Server) {
    open_guide_settings(t);
    t.open_sheet(
        "text:Add a guide source",
        "text:New guide source: link (XMLTV)",
    );
    t.wait("text:Guide sources · synthetic", 2);
    t.enter(&srv.url("/guide.xml"));
    t.wait("text:Its name (optional)", 2);
    t.enter("Main Guide");
    t.wait("text:2.  Main Guide", 5);
    t.open_sheet(
        "text:+ Add a guide source",
        "text:New guide source: link (XMLTV)",
    );
    t.enter(&srv.url("/alt.xml"));
    t.wait("text:Its name (optional)", 2);
    t.enter("Alt Guide");
    t.wait("text:3.  Alt Guide", 5);
    t.press("text:Update the guide now >> nth=1");
    t.wait("text:Guide updated", 15);
    t.wait("text:Quiet Harbour", 5);
    leave_settings(t);
}

/// The rail's Settings, its TV guide data page.
fn open_guide_settings(t: &mut T) {
    t.press("text:☰  All channels");
    t.key("left");
    t.key("left");
    t.press("text:Settings >> nth=0");
    t.press("text:TV guide data");
    t.wait("text:Update when the app starts", 2);
}

/// Back: the guide sources, the settings page, the settings; then from
/// the rail to the guide, on channel one.
fn leave_settings(t: &mut T) {
    for _ in 0..3 {
        t.key("backspace");
    }
    t.gone("text:TV guide data", 2);
    t.key("right");
    t.key("right");
}

/// Channel Three's chooser (the guide's focus on channel one).
fn open_three(t: &mut T) {
    t.key("down");
    t.key("down");
    t.key("m");
    t.press("text:Choose guide data…");
    t.wait(TITLE, 2);
}

/// Waits for the chooser to close and puts the guide's focus back on
/// channel one.
fn back_to_one(t: &mut T) {
    t.gone(TITLE, 5);
    t.key("up");
    t.key("up");
}

fn lists_and_narrows(t: &mut T) {
    open_three(t);
    for text in [
        "Now: no guide channel matches it",
        "Tap the field to search, a row to choose it",
        "Showing every guide channel",
        "5 shown · OK to switch",
        "Automatic",
        "Matched by its guide id and name",
        "No guide data",
        "The guide shows no programs for it",
        "one · Main Guide · in use",
        "two · Main Guide · in use",
        "spare · Main Guide",
        "alt-news · Alt Guide",
        "alt-one · Alt Guide",
    ] {
        assert!(t.shows(&format!("text:{text}")), "{text}");
    }
    // Automatic is the current choice; the sources in their order.
    assert_eq!(t.app.screen().count("text:Current").unwrap(), 1);
    let texts = t.texts();
    let at = |s: &str| texts.iter().position(|t| t == s).unwrap();
    assert!(at("Automatic") < at("Current") && at("Current") < at("No guide data"));
    assert!(at("Guide One") < at("Spare Channel") && at("Spare Channel") < at("Alt News"));
    // The filter: only what no channel uses.
    t.press("text:Showing every guide channel");
    t.wait("text:Showing only guide channels no channel uses", 2);
    t.wait("text:3 shown · OK to switch", 2);
    assert!(!t.shows("text:Guide One"));
    // Typing narrows it; Return goes to the first match, OK chooses it.
    t.replace("ALT");
    t.wait("text:2 shown · OK to switch", 2);
    assert!(!t.shows("text:Spare Channel"));
    t.submit();
    t.key("enter");
    // Nothing stored for it yet: the guide downloads, then shows it.
    t.wait("text:Saved; the guide is downloading to show it", 5);
    back_to_one(t);
    t.wait("text:Alt Bulletin", 15);
}

fn choices(t: &mut T, database: &std::path::Path) {
    open_three(t);
    t.wait("text:Now: Alt News (Alt Guide, chosen by you)", 2);
    assert!(t.shows("text:alt-news · Alt Guide · in use"));
    // Keys: the hint; the current choice again closes it, saving nothing.
    t.key("pagedown");
    t.wait("text:▲▼ move   OK choose   Back close", 2);
    t.key("pageup");
    t.key("=");
    // Down to Alt News (the eighth row), the current one.
    for _ in 0..7 {
        t.key("down");
    }
    t.key("enter");
    back_to_one(t);
    assert!(t.shows("text:Alt Bulletin"));

    // A guide channel another one uses: shown at once.
    let harbour = t.app.screen().count("text:Quiet Harbour").unwrap();
    open_three(t);
    t.press("text:Guide One");
    t.wait("text:Guide data changed", 5);
    back_to_one(t);
    t.count("text:Quiet Harbour", harbour + 1, 5);
    assert!(!t.shows("text:Alt Bulletin"));
    t.gone("text:Guide data changed", 15);

    // No guide data.
    open_three(t);
    t.wait("text:Now: Guide One (Main Guide, chosen by you)", 2);
    let held = iptv_app::hold_writer();
    assert!(held.wait_entered(std::time::Duration::from_secs(5)));
    t.press("text:No guide data");
    t.key("backspace");
    back_to_one(t);
    open_three(t);
    held.release();
    let result = t.app.wait_until(
        "old guide choice committed",
        std::time::Duration::from_secs(10),
        |_| {
            crate::isolated::open_database(database).is_ok_and(|store| {
                store.sources().is_ok_and(|sources| {
                    sources.first().is_some_and(|source| {
                        store.epg_overrides(source.id).is_ok_and(|overrides| {
                            overrides.iter().any(|(_, choice)| {
                                matches!(choice, iptv_store_sqlite::EpgOverride::Disabled)
                            })
                        })
                    })
                })
            })
        },
    );
    t.ok("old guide choice committed", result);
    assert!(
        t.shows(TITLE),
        "old assignment completion cannot close a reopened picker"
    );
    t.key("backspace");
    back_to_one(t);

    // A tap beside the panel closes it; Back ends typing, then closes.
    open_three(t);
    t.wait("text:Now: no guide data (chosen by you)", 2);
    t.press_beside("Automatic");
    back_to_one(t);
    open_three(t);
    for _ in 0..4 {
        t.key("up");
    }
    t.key("enter");
    let r = t.app.back();
    t.ok("back", r);
    assert!(t.shows(TITLE), "Back ended typing only");
    let r = t.app.back();
    t.ok("back", r);
    back_to_one(t);

    // Automatic again: downloaded and matched.
    open_three(t);
    t.press("text:Automatic");
    t.wait("text:Saved; the guide is downloading to show it", 5);
    back_to_one(t);
}

/// Every source switched off (Settings → TV guide data → Guide sources):
/// once the edits settle the stored guide goes, and with it the chooser.
fn every_source_off(t: &mut T) {
    open_guide_settings(t);
    t.press("text:Guide sources");
    t.wait("text:Guide sources · synthetic", 2);
    for row in [
        "1.  The provider's guide",
        "2.  Main Guide",
        "3.  Alt Guide",
    ] {
        t.press(&format!("text:{row}"));
        t.press("text:Switch off");
        t.wait("text:Switch on", 5);
        t.key("backspace");
        t.gone("text:Switch on", 2);
    }
    leave_settings(t);
    t.gone("text:Quiet Harbour", 15);
    t.key("m");
    t.press("text:Choose guide data…");
    t.wait(
        "text:No guide data is stored yet: update the guide first",
        2,
    );
    assert!(!t.shows(TITLE));
}

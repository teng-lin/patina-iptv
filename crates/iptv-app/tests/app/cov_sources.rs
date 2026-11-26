//! The playlists screen and a playlist's guide sources screen, headless,
//! by touch and by keys: what each row, sheet and notice says, and what
//! every action changes in the store (a rename, new provider details that
//! must import before they replace the old ones, a User-Agent, guide
//! sources added, refused, switched off, shifted, moved, renamed, given a
//! new link and removed, downloads and their failures, deleting a
//! playlist and then the last one).
//!
//! Synthetic data only: a playlist picked from a file and one by link,
//! their guides XMLTV served by a local HTTP server (`cov_sources/`).

use nsh_core::{Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;

use crate::support::{self, Server, T};

/// "Updated 14:05" and the like.
/// A time of day as the scenarios' locale (`en-US`) writes it: 12-hour.
const TIME: &str = r"\d{1,2}:\d\d [AP]M";

fn playlist(guide: &str) -> String {
    format!(
        "#EXTM3U url-tvg=\"{guide}\"\n\
         #EXTINF:-1 tvg-id=\"one\" group-title=\"News\",Test Channel One\nhttp://stream.example.test/1.m3u8\n\
         #EXTINF:-1 tvg-id=\"two\" group-title=\"News\",Test Channel Two\nhttp://stream.example.test/2.m3u8\n"
    )
}

fn linked(channels: usize) -> String {
    let mut m3u = String::from("#EXTM3U\n");
    for n in 1..=channels {
        m3u.push_str(&format!(
            "#EXTINF:-1 group-title=\"Sport\",Test Link Channel {n}\nhttp://stream.example.test/l{n}.m3u8\n"
        ));
    }
    m3u
}

#[test]
fn playlists_and_their_guide_sources() {
    let Some(_home) = isolated!() else { return };
    let now = support::now();
    let srv = Server::start(&[
        (
            "/guide.xml",
            support::guide(
                now,
                &[
                    ("one", "One", "Quiet Harbour"),
                    ("two", "Two", "Golden Engine"),
                ],
            ),
        ),
        (
            "/alt.xml",
            support::guide(now, &[("one", "Alt One", "Alt Morning")]),
        ),
        ("/link.m3u", linked(1)),
        ("/link2.m3u", linked(2)),
    ]);
    let config = EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    let mut t = T::new(config);
    let picked = playlist(&srv.url("/guide.xml"));
    t.app.on_pick(move |_| support::picked(picked.clone()));
    t.wait("id:wizard-welcome", 5);
    t.press("id:wizard-welcome-add");
    t.press("id:wizard-source-type");
    t.press("text:Playlist file");
    t.press("text:Choose a file…");
    // File selection completes asynchronously; submit only after the
    // selected file has reached the wizard's state and rendered.
    t.wait("text:synthetic.m3u", 5);
    t.press("text:Add playlist");
    t.wait("text:Playlist added", 30);
    t.press("text:Watch");
    t.wait("text:Golden Engine", 30);

    first_playlist(&mut t);
    add_by_link(&mut t, &srv);
    file_playlist_actions(&mut t);
    provider_details(&mut t, &srv);
    user_agent_refresh_and_keys(&mut t);
    guide_sources(&mut t, &srv);
    deleting(&mut t);
}

#[test]
fn manage_playlists_is_offered_for_hidden_sources_and_disappears_when_empty() {
    use crate::personalization::{M3U, step};
    use iptv_providers::{
        SecretUrl,
        model::{M3uSourceConfig, Source, SourceConfig},
    };
    use std::time::Duration;

    let Some(home) = isolated!() else { return };
    let dir = home.data_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    let playlist = home.path().join("synthetic.m3u");
    std::fs::write(&playlist, M3U).unwrap();
    let source = store
        .add_source(&Source {
            name: "Synthetic personalization".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new(playlist.to_string_lossy()),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    store
        .set_source_policy(
            source,
            iptv_store_sqlite::SourcePolicy {
                refresh_on_start: false,
                ..Default::default()
            },
        )
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(M3U);
    store
        .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
        .unwrap();
    store.set_all_playlists(true).unwrap();
    drop(store);

    let mut app = crate::headless::Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    step(app.wait_for("id:guide-header-details >> text:Synthetic personalization · News"));
    step(app.open_settings_playlists());
    let manage = "id:settings-content >> text:Manage playlists";
    let row = "id:settings-content >> role:checkbox[/^Synthetic personalization/]";
    step(app.wait_for(manage));
    step(app.press(row));
    step(app.wait_until(
        "the sole playlist is hidden",
        Duration::from_secs(5),
        |screen| {
            screen
                .accessibility_state(row)
                .is_ok_and(|state| state.checked == Some(nsh_core::Checked::False))
        },
    ));
    step(app.wait_for(manage));
    step(app.open_playlist_actions("Synthetic personalization"));
    step(app.press("id:playlist-actions-list >> text:Delete"));
    step(app.press("id:playlist-delete-confirmation >> text:Delete"));
    step(app.wait_for_gone(manage, Duration::from_secs(5)));
    step(app.wait_for("id:settings-content >> text:Add a playlist"));
    assert!(
        crate::isolated::open_database(path)
            .unwrap()
            .sources()
            .unwrap()
            .is_empty()
    );
}

/// Settings → Playlists.
fn open_playlists(t: &mut T) {
    let opened = t.app.open_settings_playlists();
    t.ok("Settings → Playlists", opened);
}

/// Playlist `name`'s actions: a long press on its row there.
fn actions(t: &mut T, name: &str) {
    let opened = t.app.open_playlist_actions(name);
    t.ok(&format!("{name}'s actions"), opened);
}

/// A playlist row's note ending with `status` (its summary first).
fn note(status: &str) -> String {
    format!("text:/ · {status}( · On screen)?$/")
}

/// One playlist: its row, no "Refresh all".
fn first_playlist(t: &mut T) {
    open_playlists(t);
    t.wait("text:synthetic", 5);
    assert!(t.shows(&format!(
        "text:/^M3U playlist · 2 channels · Updated {TIME} · On screen$/"
    )));
    assert!(t.shows("text:Add a playlist"));
    assert!(t.shows("text:Refresh all playlists now"));
    assert!(!t.shows("text:Reorder playlists"), "one playlist");
    t.rebase();
}

/// "Add a playlist" opens the wizard over the settings; a playlist by
/// link, named, is shown once added.
fn add_by_link(t: &mut T, srv: &Server) {
    t.press("id:settings-content >> text:Add a playlist");
    t.press("id:wizard-field-PlaylistUrl");
    t.enter(&srv.url("/link.m3u"));
    t.wait(&format!("text:{}", srv.url("/link.m3u")), 5);
    t.press("id:wizard-options");
    t.press("text:Optional: how the playlist is listed");
    t.enter("Linked");
    t.press("text:Add playlist");
    t.wait("text:Playlist added", 30);
    t.press("text:Watch");
    t.wait("text:Test Link Channel 1 · Linked", 10);
    open_playlists(t);
    t.wait("text:Linked", 5);
    // Two playlists: they can be ordered and arranged.
    assert!(t.shows("text:Playlist order"));
    assert!(t.shows("text:Reorder playlists"));
    t.count(&note(&format!("Updated {TIME}")), 2, 5);
    // These management journeys explicitly inspect an individual source.
    actions(t, "Linked");
    t.press("text:Show this playlist");
    t.wait("text:Test Link Channel 1", 10);
    open_playlists(t);
}

/// A file's actions (no provider details), a tap beside them, renaming.
fn file_playlist_actions(t: &mut T) {
    actions(t, "synthetic");
    t.wait("text:Refresh now", 5);
    for text in [
        "Guide: the provider's own",
        "Show this playlist",
        "Rename",
        "User-Agent",
        "Guide data",
        "Delete",
    ] {
        assert!(t.shows(&format!("text:{text}")), "{text}");
    }
    assert!(
        !t.shows("text:Playlist link"),
        "a file has no link to change"
    );
    t.press_beside("Refresh now");
    t.gone("text:Refresh now", 2);

    actions(t, "synthetic");
    t.open_sheet("text:Rename", "text:Playlist name");
    assert_eq!(t.field(), "synthetic");
    t.enter("Synthetic Renamed");
    t.wait("text:Synthetic Renamed", 5);
    t.gone("text:Playlist name", 2);
    assert!(!t.shows("text:synthetic"));
    // An empty name keeps the old one; Cancel changes nothing.
    actions(t, "Synthetic Renamed");
    t.open_sheet("text:Rename", "text:Playlist name");
    t.enter("");
    t.wait("text:A playlist needs a name. The old one is kept.", 2);
    t.open_sheet("text:Rename", "text:Playlist name");
    t.replace("Never Saved");
    t.press("text:Cancel");
    t.gone("text:Playlist name", 2);
    assert!(!t.shows("text:Never Saved"));
    // Back closes the actions.
    t.key("backspace");
    t.gone("text:Refresh now", 2);
    assert!(t.shows("text:Synthetic Renamed"));
}

/// New provider details are checked by importing them: refused, the old
/// ones stay; working, they replace them.
fn provider_details(t: &mut T, srv: &Server) {
    let link = srv.url("/link.m3u");
    actions(t, "Linked");
    t.wait("text:On screen now: back to the guide", 2);
    assert!(t.shows("text:Guide: none"));
    t.press("text:Playlist link");
    t.wait("text:Check and save", 2);
    assert!(t.shows(
        "text:New details are tried first: the playlist keeps its old ones and its channels unless the new ones work."
    ));
    assert!(t.shows(&format!("text:{link}")));
    t.press("text:Check and save");
    t.wait("text:Nothing was changed.", 2);
    // A tap beside the form keeps it (its edits would be lost).
    t.press_beside("Check and save");
    assert!(t.shows("text:Check and save"));
    // The link row: its text to change; a malformed link stays beside it.
    t.open_sheet("text:Playlist link >> nth=1", "text:Playlist link >> nth=2");
    assert_eq!(t.field(), link);
    t.enter("ftp://bad.example.test/x.m3u");
    t.wait("text:Only http:// and https:// links work", 2);
    t.open_sheet("text:Playlist link >> nth=1", "text:Playlist link >> nth=2");
    t.enter(&srv.url("/missing.m3u"));
    t.press("text:Check and save");
    // The row behind says why, and keeps the old channels.
    t.wait(
        &note("New details not saved: the answer was not a playlist\\. The old ones stay\\."),
        15,
    );
    assert!(t.shows("text:Check and save"), "refused: the form stays");
    assert!(
        t.shows("text:/^M3U playlist · 1 channel · /"),
        "the old channels"
    );
    // By keys: Down to "Check and save", Up to the link, OK types.
    t.key("down");
    t.wait(
        "text:▲▼ choose   OK change or check   Back leave unchanged",
        2,
    );
    t.key("up");
    t.key("enter");
    assert_eq!(t.field(), srv.url("/missing.m3u"));
    t.enter(&srv.url("/link2.m3u"));
    t.press("text:Check and save");
    t.gone("text:Check and save", 15);
    t.count("text:/^M3U playlist · 2 channels · /", 2, 10);
    t.wait("text:Test Link Channel 2", 10);
    // Back from the form leaves it unchanged.
    actions(t, "Linked");
    t.press("text:Playlist link");
    t.wait("text:Check and save", 2);
    t.key("backspace");
    t.gone("text:Check and save", 2);
    t.wait("text:User-Agent", 2);
    t.key("backspace");
    t.gone("text:User-Agent", 2);
}

/// A User-Agent is tried like new details and kept; refreshing; the list
/// by keys.
fn user_agent_refresh_and_keys(t: &mut T) {
    actions(t, "Linked");
    t.open_sheet("text:User-Agent", "text:User-Agent >> nth=1");
    t.wait(
        "text:Empty: the app's own; some providers ask for a particular one",
        2,
    );
    assert_eq!(t.field(), "");
    t.enter("Synthetic/1");
    t.gone("text:Cancel", 2);
    // Checked and saved: the actions close, the row is updated again.
    t.count(&note(&format!("Updated {TIME}")), 2, 15);
    assert!(!t.shows("text:/Checking the new details/"));
    actions(t, "Linked");
    t.open_sheet("text:User-Agent", "text:User-Agent >> nth=1");
    t.wait("text:Cancel", 2);
    assert_eq!(t.field(), "Synthetic/1", "the User-Agent was kept");
    t.press("text:Cancel");
    // Streams at once (#282): Automatic, then an explicit one, shown at
    // once though it equals the default ceiling, then on.
    t.press("text:Streams at once: Automatic");
    t.wait("text:Streams at once: 1", 2);
    t.press("text:Streams at once: 1");
    t.wait("text:Streams at once: 2", 2);
    t.press("text:Refresh now");
    t.gone("text:Show this playlist", 2);
    t.gone("id:playlists-task", 5);
    t.press("text:Refresh all playlists now");
    // Both refresh, then read as updated again.
    t.count(&note(&format!("Updated {TIME}")), 2, 20);

    // Keys: the focus pill, the hints, Menu opens a playlist's actions.
    // Two playlist rows, then Add, then Refresh all (focused).
    for _ in 0..3 {
        t.key("up");
    }
    t.wait(
        "text:▲▼ move   OK show or hide   Menu its settings   ◀ Back pages",
        2,
    );
    let menu = t.app.press_remote(nsh_tv::RemoteKey::Menu);
    t.ok("key menu", menu);
    t.wait("id:playlist-menu-panel >> text:Hide", 2);
    t.key("backspace");
    actions(t, "Synthetic Renamed");
    t.press("text:Show this playlist");
    t.wait("text:Golden Engine", 15);
}

/// The guide sources of the file playlist (shown again).
fn guide_sources(t: &mut T, srv: &Server) {
    actions(t, "Synthetic Renamed");
    t.press("text:Guide data");
    t.wait("text:Guide data · Synthetic Renamed", 5);
    t.wait(
        "text:Tap a source for what you can do with it. The first source with a channel wins",
        2,
    );
    let guide = srv.url("/guide.xml");
    for text in [
        "1.  The provider's guide".to_owned(),
        format!("From the provider · {guide}"),
        "+ Add a guide source".to_owned(),
        "An XMLTV link (.xml or .xml.gz)".to_owned(),
        "Update the guide now".to_owned(),
        "Downloads every source switched on".to_owned(),
    ] {
        assert!(t.shows(&format!("text:{text}")), "{text}");
    }
    assert!(t.shows(&format!(
        "text:/^2 channels used · 2 programs · updated {TIME}$/"
    )));
    assert!(t.shows(&format!(
        "text:/^Last update {TIME} · 2 channels with a guide$/"
    )));

    provider_source(t);
    adding(t, srv);
    let alt = srv.url("/alt.xml");
    // Update now: the failure is the missing one's; the guide says so.
    t.press("text:Update the guide now");
    t.wait("text:Guide updated", 15);
    t.wait(
        &format!("text:/^Failed {TIME}: the server turned the request down$/"),
        5,
    );
    t.wait(
        &format!("text:/^The last update {TIME} did not load every source$/"),
        5,
    );
    t.wait(
        &format!("text:/^Its 1 channel is not used here · updated {TIME}$/"),
        5,
    );
    user_source(t, srv, &alt);
    nothing_switched_on(t);
    // Back: the screen closes over the playlist's actions.
    t.key("backspace");
    t.gone("text:Guide data · Synthetic Renamed", 2);
    t.wait("text:Rename", 2);
    t.key("backspace");
    t.gone("text:Rename", 2);
}

/// The provider's own guide: no new link, no removal; its time shift by
/// keys.
fn provider_source(t: &mut T) {
    t.press("text:1.  The provider's guide");
    t.wait("text:Switch off", 2);
    for text in ["Move up (used first)", "Move down", "No time shift", "Name"] {
        assert!(t.shows(&format!("text:{text}")), "{text}");
    }
    assert!(!t.shows("text:Change the link") && !t.shows("text:Remove"));
    for _ in 0..3 {
        t.key("down");
    }
    t.wait(
        "text:▲▼ choose   OK do it   ◀▶ shift times   Back close the sheet",
        2,
    );
    t.key("right");
    t.wait("text:Times +0:30 h", 5);
    t.key("left");
    t.wait("text:No time shift", 5);
    // Left and Right do nothing on another action.
    t.key("up");
    t.key("right");
    assert!(t.shows("text:No time shift"));
    t.key("backspace");
    t.gone("text:Switch off", 2);
    t.wait(
        "text:▲▼ choose   OK actions   Back close   The first source with a channel wins",
        2,
    );
}

/// "Add a guide source": a link (refused when malformed or already
/// there, offered again), then a name.
fn adding(t: &mut T, srv: &Server) {
    t.open_sheet(
        "text:+ Add a guide source",
        "text:New guide source: link (XMLTV)",
    );
    t.press("text:Cancel");
    t.gone("text:New guide source: link (XMLTV)", 2);
    t.open_sheet(
        "text:+ Add a guide source",
        "text:New guide source: link (XMLTV)",
    );
    t.enter("ftp://bad.example.test/x.xml");
    t.wait("text:Only http:// and https:// links work", 2);
    t.open_sheet(
        "text:+ Add a guide source",
        "text:New guide source: link (XMLTV)",
    );
    assert_eq!(t.field(), "ftp://bad.example.test/x.xml", "offered again");
    t.enter(&srv.url("/guide.xml"));
    t.wait("text:This playlist has that guide already.", 2);
    t.open_sheet(
        "text:+ Add a guide source",
        "text:New guide source: link (XMLTV)",
    );
    t.enter("");
    t.wait("text:Type the guide's link (XMLTV).", 2);
    t.open_sheet(
        "text:+ Add a guide source",
        "text:New guide source: link (XMLTV)",
    );
    t.enter(&srv.url("/alt.xml"));
    t.wait("text:Its name (optional)", 2);
    t.enter("Alt Guide");
    t.wait("text:2.  Alt Guide", 5);
    assert!(t.shows(&format!("text:Added by you · {}", srv.url("/alt.xml"))));
    // Unnamed: its link is its title.
    t.open_sheet(
        "text:+ Add a guide source",
        "text:New guide source: link (XMLTV)",
    );
    t.enter(&srv.url("/missing.xml"));
    t.wait("text:Its name (optional)", 2);
    t.enter("");
    t.wait(&format!("text:3.  {}", srv.url("/missing.xml")), 5);
}

/// A source the user added: every action.
fn user_source(t: &mut T, srv: &Server, alt: &str) {
    t.press("text:2.  Alt Guide");
    t.wait("text:Remove", 2);
    assert!(t.shows(&format!("text:Added by you · {alt} >> nth=1")));
    t.press("text:Switch off");
    t.wait("text:Switch on", 5);
    t.wait("text:Off", 2);
    assert!(t.shows("text:Switched off: not downloaded"));
    t.press("text:+");
    t.wait("text:Times +0:30 h", 5);
    t.press("text:−");
    t.wait("text:No time shift", 5);
    t.press("text:−");
    t.wait("text:Times −0:30 h", 5);
    t.press("text:Move up (used first)");
    t.wait("text:1.  Alt Guide", 5);
    assert!(t.shows("text:2.  The provider's guide"));
    t.press("text:Move down");
    t.wait("text:2.  Alt Guide", 5);
    t.open_sheet("text:Name", "text:Name >> nth=1");
    assert_eq!(t.field(), "Alt Guide");
    t.enter("Alt Renamed");
    t.wait("text:2.  Alt Renamed", 5);
    // A new link: refused when the playlist has it; empty keeps it.
    t.open_sheet("text:Change the link", "text:New link (XMLTV)");
    assert_eq!(t.field(), alt);
    t.enter(&srv.url("/guide.xml"));
    t.wait("text:This playlist has that guide already.", 2);
    t.open_sheet("text:Change the link", "text:New link (XMLTV)");
    t.enter("");
    t.gone("text:New link (XMLTV)", 2);
    assert!(t.shows(&format!("text:Added by you · {alt} >> nth=1")));
    // Removing asks first; keeping it is the default.
    t.press("text:Remove");
    t.wait("text:Remove “Alt Renamed”?", 2);
    assert!(t.shows(
        "text:“Alt Renamed” will no longer be downloaded. Its programs go away at the next guide update."
    ));
    t.key("enter");
    t.gone("text:Remove “Alt Renamed”?", 2);
    t.press("text:Remove");
    t.key("down");
    t.wait("text:▲▼ choose   OK confirm   Back keep it", 2);
    t.key("backspace");
    t.gone("text:Remove “Alt Renamed”?", 2);
    t.press("text:Remove");
    t.press("text:Keep it");
    t.gone("text:Remove “Alt Renamed”?", 2);
    t.press("text:Remove");
    t.press("text:Remove");
    t.gone("text:2.  Alt Renamed", 5);
    assert!(!t.shows("text:Remove"), "the sheet closed");
    t.wait(&format!("text:2.  {}", srv.url("/missing.xml")), 2);
}

/// Every source off: nothing to download.
fn nothing_switched_on(t: &mut T) {
    for row in ["text:1.  The provider's guide", "text:/^2\\.  http:/"] {
        t.press(row);
        t.press("text:Switch off");
        t.wait("text:Switch on", 5);
        t.press_beside("Switch on");
        t.gone("text:Switch on", 2);
    }
    let update = if t.shows("text:Retry") {
        "text:Retry"
    } else {
        "text:Update the guide now"
    };
    t.press(update);
    t.wait("text:No guide source is switched on", 5);
    // On again, by keys: Menu on the first row, OK on "Switch on".
    t.key("up");
    t.key("up");
    t.key("up");
    let menu = t.app.press_remote(nsh_tv::RemoteKey::Menu);
    t.ok("key menu", menu);
    t.wait("text:Switch on", 2);
    t.key("enter");
    t.wait("text:Switch off", 5);
    t.key("backspace");
    t.gone("text:Switch off", 2);
}

/// Deleting asks first and says what goes; the last one leaves the
/// first-run screen.
fn deleting(t: &mut T) {
    t.press("id:settings-content >> role:button[More options for Linked]");
    t.press("id:playlist-menu-panel >> text:Delete");
    t.wait("text:Delete “Linked”?", 2);
    assert!(t.shows(
        "text:Its 2 channels, its guide and your favorites from it will be gone from the app. The playlist at your provider is not affected."
    ));
    t.press("text:Keep it");
    t.gone("text:Delete “Linked”?", 2);
    t.press("id:playlist-menu-panel >> text:Delete");
    t.press("text:Delete");
    t.gone("text:Linked", 5);
    assert!(!t.shows("text:Reorder playlists"), "one playlist left");
    actions(t, "Synthetic Renamed");
    t.press("text:Delete");
    t.wait("text:/No playlists will be left\\.$/", 2);
    t.press("text:Delete");
    t.wait("id:wizard-welcome", 10);
    assert!(!t.shows("text:Tap a playlist for what you can do with it"));
}

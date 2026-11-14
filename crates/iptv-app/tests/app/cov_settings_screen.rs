//! The guide's menu, the favourites editor and the settings screen,
//! headless: the menu's items (search, favourites, details, guide data,
//! play from the beginning, disabled items), the favourites editor by
//! keys and taps, and every kind of settings row and editor (a switch, a
//! choice with swatches, a stepper, a text with a rejected value, a
//! confirmation, the remote's key list, commands that open other screens
//! or update the guide), what they store, and the second Back that
//! leaves.
//!
//! Synthetic data only: a playlist "picked" from a file, with a local
//! XMLTV guide.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iptv_epg::format_xmltv_time;
use nsh_core::Checked;
use nsh_core::picker::PickedFile;
use nsh_core::{ImageData, Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;
use nsh_testing::{Headless, Screen};

const IMPORT: Duration = Duration::from_secs(30);
const GONE: Duration = Duration::from_secs(3);
const DESCRIPTION: &str =
    "A synthetic programme description for testing the guide. Real descriptions come from the EPG.";

#[track_caller]
fn step(r: Result<(), String>) {
    if let Err(e) = r {
        panic!("{e}");
    }
}

fn guide(now: i64) -> String {
    let mut xml = String::from("<?xml version=\"1.0\"?><tv>");
    for (id, title) in [("one", "Quiet Harbour"), ("two", "Golden Engine")] {
        xml.push_str(&format!(
            "<channel id=\"{id}\"><display-name>{id}</display-name></channel>\
             <programme channel=\"{id}\" start=\"{}\" stop=\"{}\"><title>{title}</title></programme>",
            format_xmltv_time(now - 1800),
            format_xmltv_time(now + 1800)
        ));
    }
    xml + "</tv>"
}

/// The saved settings file under `home`.
fn settings_file(home: &Path) -> Option<PathBuf> {
    let mut dirs = vec![home.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.file_name().is_some_and(|n| n == "settings.json") {
                return Some(path);
            }
        }
    }
    None
}

/// The saved settings, once they hold `want`.
fn saved(app: &mut Headless, home: &Path, want: &str) {
    let read = || {
        settings_file(home)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default()
    };
    let found =
        crate::isolated::wait_outside(app, "settings saved", Duration::from_secs(5), || {
            read().contains(want)
        });
    assert!(found.is_ok(), "{want} in {}", read());
}

#[test]
fn menus_favourites_and_settings() {
    let Some(home) = isolated!() else { return };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let guide_path = home.path().join("guide.xml");
    std::fs::write(&guide_path, guide(now)).unwrap();
    let playlist = format!(
        "#EXTM3U url-tvg=\"{}\"\n\
         #EXTINF:-1 tvg-id=\"one\" catchup=\"shift\" catchup-days=\"3\" group-title=\"News\",Test Channel One\nhttp://stream.example.test/1.m3u8\n\
         #EXTINF:-1 tvg-id=\"two\" group-title=\"News\",Test Channel Two\nhttp://stream.example.test/2.m3u8\n",
        guide_path.display()
    );
    let config = EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    let mut app = Headless::start(config).unwrap();
    app.on_pick(move |_| {
        vec![PickedFile::new(
            "synthetic.m3u",
            "audio/x-mpegurl",
            ImageData::new(playlist.as_bytes().to_vec()),
        )]
    });
    step(app.wait_for("text:No playlists yet"));
    step(app.press("text:Add playlist"));
    step(app.wait_for("text:Playlist file"));
    step(app.press("text:Playlist file"));
    step(app.wait_for("text:Choose a file…"));
    step(app.press("text:Choose a file…"));
    step(app.wait_for("text:synthetic.m3u"));
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", IMPORT));
    step(app.press("text:Watch"));
    step(app.wait_for_within("text:Golden Engine", IMPORT));

    menu(&mut app);
    favourites(&mut app);
    settings(&mut app, home.path());
    leaving(&mut app);

    // Last (its field keeps the keyboard focus once it hides): keys in the
    // menu, Down to the next item and back, OK on Search opens it.
    step(app.press_key("right"));
    step(app.press_key("right"));
    open_menu(&mut app);
    step(app.press_key("down"));
    step(app.press_key("up"));
    step(app.press_key("enter"));
    step(app.wait_for("text:No recent searches yet"));
    step(app.press("text:Close"));
    step(app.wait_for_gone("text:No recent searches yet", GONE));
}

/// Back (Escape) until `done` holds: a screen over another may first end
/// typing in its field.
#[track_caller]
fn back_until(app: &mut Headless, done: impl Fn(&Screen) -> bool) {
    for _ in 0..4 {
        if done(app.screen()) {
            return;
        }
        step(app.back());
        step(app.wait(Duration::from_millis(150)));
    }
    assert!(done(app.screen()), "still open: {:?}", app.screen().texts());
}

/// Menu (`M`) on the focused programme.
#[track_caller]
fn open_menu(app: &mut Headless) {
    step(app.press_key("m"));
    step(app.wait_for("text:Manage favourites…"));
}

fn menu(app: &mut Headless) {
    // The menu names the programme, the channel and the list; items the
    // app cannot do yet do nothing.
    open_menu(app);
    assert!(app.screen().matches("role:menuitem").unwrap());
    step(app.wait_for("text:/^Quiet Harbour · /"));
    step(app.wait_for("text:Test Channel One >> nth=1"));
    step(app.press("text:Block channel"));
    step(app.wait(Duration::from_millis(100)));
    assert!(app.shows("Block channel"), "a disabled item keeps the menu");
    // Favourites: added, and the menu then offers to remove it.
    step(app.press("text:Add to favourites"));
    step(app.wait_for_gone("text:Manage favourites…", GONE));
    open_menu(app);
    step(app.wait_for("text:Remove from favourites"));
    // Menu again closes it.
    step(app.press_key("m"));
    step(app.wait_for_gone("text:Remove from favourites", GONE));

    // Details: the description over the guide; Back closes it, as does a
    // tap.
    open_menu(app);
    step(app.press("text:Details"));
    step(app.wait_for(&format!("text:{DESCRIPTION}")));
    step(app.wait_for("text:/^Test Channel One · /"));
    step(app.press_key("backspace"));
    step(app.wait_until("details hidden", GONE, |s| {
        !s.texts()
            .iter()
            .any(|t| t.starts_with("Test Channel One · "))
    }));
    open_menu(app);
    step(app.press("text:Details"));
    step(app.wait_for("text:/^Test Channel One · /"));
    step(app.press_key("m"));
    step(app.wait_until("details hidden", GONE, |s| {
        !s.texts()
            .iter()
            .any(|t| t.starts_with("Test Channel One · "))
    }));
    open_menu(app);
    step(app.press("text:Details"));
    step(app.wait_for("text:/^Test Channel One · /"));
    step(app.press(&format!("text:{DESCRIPTION}")));
    step(app.wait_until("details hidden", GONE, |s| {
        !s.texts()
            .iter()
            .any(|t| t.starts_with("Test Channel One · "))
    }));

    // Guide data for the channel: its chooser; Back closes it.
    open_menu(app);
    step(app.press("text:Choose guide data…"));
    step(app.wait_for("text:Guide data for Test Channel One"));
    assert!(!app.shows("Manage favourites…"));
    // Back ends typing in its search field, then closes it.
    back_until(app, |s| !s.shows("Guide data for Test Channel One"));

    // Back closes the menu.
    open_menu(app);
    step(app.back());
    step(app.wait_for_gone("text:Manage favourites…", GONE));

    // Play from the beginning: the replay, full screen; Back returns.
    open_menu(app);
    step(app.press("text:Play from the beginning"));
    step(app.wait_for_gone("text:Manage favourites…", GONE));
    step(app.wait_for("text:▶"));
    step(app.back());
    step(app.back());
    step(app.wait_for("text:☰  All channels"));
}

/// The favourites editor's title (empty while it is closed).
const EDITOR: &str = "text:/^Favourites · /";

fn favourites(app: &mut Headless) {
    // Opened on the list on screen: one of two starred (Channel One).
    open_menu(app);
    step(app.press("text:Manage favourites…"));
    step(app.wait_for("text:/^Favourites · .* · 1 of 2$/"));
    step(app.wait_for("text:Tap a channel to add or remove it"));
    assert!(app.screen().matches("role:checkbox").unwrap());
    // Keys: Left removes all, Right adds all, OK toggles the focused one.
    step(app.press_key("left"));
    step(app.wait_for("text:/ · 0 of 2$/"));
    step(app.wait_for(
        "text:OK add or remove   ◀ remove all in this list   ▶ add all in this list   Back done",
    ));
    step(app.press_key("right"));
    step(app.wait_for("text:/ · 2 of 2$/"));
    step(app.press_key("down"));
    step(app.press_key("up"));
    step(app.press_key("enter"));
    step(app.wait_for("text:/ · 1 of 2$/"));
    // Taps: a row toggles, the buttons do the whole list.
    step(app.press("text:☆"));
    step(app.wait_for("text:/ · 2 of 2$/"));
    step(app.press("text:Remove all"));
    step(app.wait_for("text:/ · 0 of 2$/"));
    step(app.press("text:Add all"));
    step(app.wait_for("text:/ · 2 of 2$/"));
    step(app.press("text:Done"));
    step(app.wait_for_gone(EDITOR, GONE));
    // Back (a key) leaves it too.
    open_menu(app);
    step(app.press("text:Manage favourites…"));
    step(app.wait_for(EDITOR));
    step(app.back());
    step(app.wait_for_gone(EDITOR, GONE));
}

/// The text field of a text sheet, if one is open.
fn field_id(screen: &Screen) -> Option<u32> {
    screen
        .layout_nodes()
        .into_iter()
        .filter(|n| matches!(n.kind, nsh_core::ViewKind::Input) && !n.hidden)
        .map(|n| n.id)
        .next_back()
}

/// The text sheet's field, as a selector, once it is open.
#[track_caller]
fn input(app: &mut Headless) -> String {
    step(app.wait_until("the text sheet", GONE, |s| field_id(s).is_some()));
    format!("view:{}", field_id(app.screen()).unwrap())
}

/// Presses the text sheet's button `label` (the favourites editor, mounted
/// while closed, has a Done too).
#[track_caller]
fn sheet_button(app: &mut Headless, label: &str) {
    let field = field_id(app.screen()).expect("the text sheet");
    let nodes = app.screen().layout_nodes();
    let parent = |id: u32| nodes.iter().find(|n| n.id == id).map(|n| n.parent);
    let inside = |mut id: u32, around: u32| {
        while let Some(up) = parent(id) {
            if up == around {
                return true;
            }
            id = up;
        }
        false
    };
    let mut around = field;
    while let Some(up) = parent(around) {
        around = up;
        let holds = nodes.iter().any(|n| {
            n.text.as_ref().is_some_and(|t| *t.text.text == *label) && inside(n.id, around)
        });
        if holds {
            break;
        }
    }
    step(app.press(&format!("view:{around} >> text:{label}")));
}

#[track_caller]
fn sheet_closed(app: &mut Headless) {
    step(app.wait_until("the text sheet closed", GONE, |s| field_id(s).is_none()));
}

/// Waits for the settings row `label` to read `value`.
fn row_value(app: &mut Headless, label: &str, value: &str) {
    let (label, value) = (label.to_owned(), value.to_owned());
    step(app.wait_until(
        &format!("{label} reading {value}"),
        Duration::from_secs(3),
        move |s| {
            let texts = s.texts();
            texts
                .iter()
                .position(|t| *t == label)
                .is_some_and(|i| texts[i..].iter().take(4).any(|t| *t == value))
        },
    ));
}

const PAGES_HINT: &str = "text:▲▼ page   ▶ OK its settings   Back close";
const ROWS_HINT: &str = "text:▲▼ move   OK change   ◀ Back pages";
/// The menu's Settings (the player's control strip, mounted while hidden,
/// has one before it).
const MENU_SETTINGS: &str = "text:Settings >> nth=1";

fn settings(app: &mut Headless, home: &Path) {
    open_menu(app);
    step(app.press(MENU_SETTINGS));
    step(app.wait_for("text:Play the last channel on start"));
    assert!(app.screen().matches("role:tab").unwrap());
    assert!(app.screen().matches("role:switch").unwrap());
    assert!(
        !app.screen()
            .accessibility_state("role:switch[/Play the last channel on start/]")
            .unwrap()
            .selected
    );
    // The pages, by keys: the hint, and what the page offers.
    step(app.press_key("down"));
    step(app.press_key("up"));
    step(app.wait_for(PAGES_HINT));
    step(app.wait_for("text:/^\\d+ of \\d+ settings here can be changed now$/"));
    // Into the rows: backup is now the first available command. Its
    // description and Restore's are reachable before the startup switches.
    step(app.press_key("right"));
    step(app.wait_for(ROWS_HINT));
    step(app.wait_for("text:Write settings and customisations to a backup file."));
    step(app.press_key("down"));
    step(app.wait_for("text:Replace everything with a backup's contents."));
    // Unavailable command rows still take remote focus; passing them does
    // not invoke a command or open an editor.
    step(app.press_key("down"));
    step(app.press_key("down"));
    step(app.press_key("down"));
    step(app.wait_for("text:Start playing the channel that was on when the app was last closed."));
    // A failed write is visible and leaves the live setting unchanged.
    let data = crate::isolated::data_dir(home);
    let blocked = data.join("settings.json.tmp");
    std::fs::create_dir(&blocked).unwrap();
    row_value(app, "Play the last channel on start", "On");
    step(app.press_key("enter"));
    step(
        app.wait_for("text:Settings could not be saved. Check storage permissions and try again."),
    );
    row_value(app, "Play the last channel on start", "On");
    std::fs::remove_dir(&blocked).unwrap();
    // Retrying persists the setting once the write failure is removed.
    step(app.press_key("enter"));
    row_value(app, "Play the last channel on start", "Off");
    saved(app, home, "general.resume_last_channel");
    // Rows the app does not act on say so, and do nothing.
    step(app.wait_for("text:Not available yet"));
    step(app.press("text:On-screen keyboard"));
    step(app.wait(Duration::from_millis(100)));
    assert!(field_id(app.screen()).is_none(), "no editor for that row");
    step(app.wait_for("text:Tap a page, then a setting to change it"));

    // Text: the entry sheet; too long a value is refused, a good one kept.
    step(app.press("text:User-Agent"));
    let field = input(app);
    step(app.type_text(&field, &"x".repeat(513)));
    sheet_button(app, "Done");
    step(app.wait_for("text:That is not a valid value here"));
    step(app.clear(&field));
    step(app.wait_for_gone("text:That is not a valid value here", GONE));
    step(app.type_text(&field, "SyntheticPlayer/1.0\n"));
    sheet_closed(app);
    row_value(app, "User-Agent", "SyntheticPlayer/1.0");
    saved(app, home, "SyntheticPlayer/1.0");
    // Cancel and Back leave it as it was.
    step(app.press("text:User-Agent"));
    let field = input(app);
    step(app.type_text(&field, " extra"));
    sheet_button(app, "Cancel");
    sheet_closed(app);
    step(app.press("text:User-Agent"));
    let field = input(app);
    step(app.type_text(&field, " extra"));
    step(app.back());
    sheet_closed(app);
    row_value(app, "User-Agent", "SyntheticPlayer/1.0");

    appearance(app, home);
    guide_data(app, home);
    other_pages(app);

    // Back: the rows, then the pages, then the screen closes; it opens on
    // the page shown last.
    step(app.press("text:Remote control >> nth=0"));
    step(app.press_key("right"));
    step(app.wait_for(ROWS_HINT));
    step(app.back());
    step(app.wait_for(PAGES_HINT));
    step(app.press_key("left"));
    step(app.back());
    step(app.wait_for_gone(PAGES_HINT, GONE));
    open_menu(app);
    step(app.press(MENU_SETTINGS));
    step(app.wait_for("text:Edit remote bindings"));
    step(app.press_key("backspace"));
    step(app.wait_for_gone("text:Edit remote bindings", GONE));
}

fn appearance(app: &mut Headless, home: &Path) {
    // A colour: the choice at the right, the current one ticked, each with
    // its swatch; a tap picks.
    step(app.press("text:Appearance >> nth=0"));
    step(app.wait_for("text:Accent colour"));
    row_value(app, "Accent colour", "Blue");
    step(app.press("text:Accent colour >> nth=0"));
    step(app.wait_for("text:✓  Blue"));
    assert!(app.screen().matches("role:option").unwrap());
    step(app.press("text:Red"));
    step(app.wait_for_gone("text:✓  Blue", GONE));
    row_value(app, "Accent colour", "Red");
    saved(app, home, "\"red\"");
    // By keys: OK opens, Down moves, OK picks; Back cancels.
    step(app.press("text:Background colour"));
    step(app.wait_for("text:✓  Dark"));
    step(app.back());
    step(app.wait_for_gone("text:✓  Dark", GONE));
    step(app.press_key("enter"));
    step(app.wait_for("text:▲▼ choose   OK keep   Back cancel"));
    step(app.back());
    step(app.wait_for(ROWS_HINT));
    step(app.press_key("enter"));
    step(app.wait_for("text:▲▼ choose   OK keep   Back cancel"));
    step(app.press_key("down"));
    step(app.press_key("pagedown"));
    step(app.press_key("pageup"));
    step(app.press_key("enter"));
    step(app.wait_for(ROWS_HINT));

    // A number: the stepper with its range; − and + by taps and keys,
    // Cancel keeps the old value, Keep (or OK) the new one.
    let label = "Video showing through screens";
    row_value(app, label, "20 %");
    step(app.press(&format!("text:{label}")));
    step(app.wait_for("text:From 0 % to 80 %, in steps of 5 %"));
    step(app.wait_for("text:◀  20 %  ▶"));
    step(app.press("text:+"));
    step(app.wait_for("text:◀  25 %  ▶"));
    step(app.press("text:Cancel"));
    step(app.wait_for_gone("text:◀  25 %  ▶", GONE));
    row_value(app, label, "20 %");
    step(app.press(&format!("text:{label}")));
    step(app.wait_for("text:−"));
    step(app.press("text:−"));
    step(app.wait_for("text:◀  15 %  ▶"));
    step(app.press("text:Keep"));
    row_value(app, label, "15 %");
    step(app.press(&format!("text:{label}")));
    step(app.press_key("right"));
    step(app.wait_for("text:◀  20 %  ▶"));
    step(app.wait_for("text:◀▶ change   OK keep   Back cancel"));
    step(app.press_key("pageup"));
    step(app.wait_for("text:◀  70 %  ▶"));
    step(app.press_key("enter"));
    row_value(app, label, "70 %");
    saved(app, home, "appearance.ui_transparency");
}

fn guide_data(app: &mut Headless, home: &Path) {
    step(app.press("text:TV guide data >> nth=0"));
    step(app.wait_for("text:Update the guide now"));
    row_value(app, "Guide sources", "1 source");
    // Update now: the guide is downloaded again (with the User-Agent set).
    step(app.press("text:Update the guide now"));
    step(app.wait_until("the guide updated", IMPORT, |s| {
        s.texts()
            .iter()
            .any(|t| t.starts_with("Updated ") && t.ends_with(" · 2 channels"))
    }));
    // The guide file gone: the update fails, and the source says so.
    let guide_path = home.join("guide.xml");
    let kept = std::fs::read(&guide_path).unwrap();
    std::fs::remove_file(&guide_path).unwrap();
    step(app.press("text:Update the guide now"));
    step(app.wait_until("the guide update failed", IMPORT, |s| {
        s.texts().iter().any(|t| t.starts_with("Failed "))
    }));
    row_value(app, "Guide sources", "1 source · 1 failed");
    std::fs::write(&guide_path, kept).unwrap();
    // The playlist's guide sources, over the settings; Back returns.
    let sources = "text:/^Guide sources · /";
    step(app.press("text:Guide sources"));
    step(app.wait_for(sources));
    back_until(app, |s| s.matches(sources) == Ok(false));
    step(app.press("text:Add a guide source"));
    step(app.wait_for(sources));
    back_until(app, |s| s.matches(sources) == Ok(false));
    step(app.wait_for("text:Update the guide now"));
    // Delete: asked first; Cancel does nothing, the button deletes.
    step(app.press("text:Delete stored guide data"));
    step(app.wait_for("text:Delete stored guide data?"));
    step(app.wait_for(
        "text:The stored programmes are deleted. The guide stays empty until it is downloaded again.",
    ));
    step(app.press("text:Cancel"));
    step(app.wait_for_gone("text:Delete stored guide data?", GONE));
    step(app.press("text:Delete stored guide data"));
    step(app.press_key("down"));
    step(app.wait_for("text:OK go ahead   Back cancel"));
    step(app.press_key("enter"));
    step(app.wait_for_within("text:Guide data deleted", IMPORT));
    step(app.wait_for_gone("text:Guide data deleted", Duration::from_secs(6)));
}

fn other_pages(app: &mut Headless) {
    // Parental control: planned for later.
    step(app.press("text:Parental control >> nth=0"));
    step(app.wait_for("text:In a later version"));
    step(app.wait_for("text:/ settings here, none available yet$/"));
    // About: the compiled version and source commit.
    step(app.press("text:About >> nth=0"));
    step(app.wait_for(&format!(
        "text:v{}({})",
        env!("CARGO_PKG_VERSION"),
        env!("IPTV_GIT_HASH")
    )));
    // Playlists: its screen over the settings; a rename there types in
    // the wizard's text sheet.
    step(app.press("text:Playlists >> nth=0"));
    step(app.wait_for("text:Manage playlists"));
    step(app.press("text:Manage playlists"));
    let shown = |s: &Screen| s.count("text:Playlists").unwrap_or(0);
    step(app.wait_until("the playlists screen", GONE, |s| shown(s) > 1));
    step(app.press("text:synthetic"));
    step(app.wait_for("text:Rename"));
    step(app.press("text:Rename"));
    let field = input(app);
    step(app.wait_for("text:Playlist name"));
    step(app.clear(&field));
    step(app.type_text(&field, "Renamed list\n"));
    sheet_closed(app);
    step(app.wait_for("text:Renamed list"));
    back_until(app, |s| shown(s) == 1);
    // The remote's bindings: its editor opens over the settings (its own
    // test, tests/app/library_management.rs, edits them); Back closes it.
    step(app.press("text:Remote control >> nth=0"));
    step(app.wait_for("text:Edit remote bindings"));
    step(app.press("text:Edit remote bindings"));
    step(app.wait_for("text:Remote bindings"));
    step(app.back());
    step(app.wait_for_gone("id:remote-editor-screen", GONE));
    // Pages by keys: Page Down and Up move five.
    step(app.press_key("left"));
    step(app.press_key("pagedown"));
    step(app.press_key("pageup"));
    step(app.wait_for(PAGES_HINT));
}

/// With "Press Back twice to leave" (on by default), the first Back at
/// the top asks for a second.
fn leaving(app: &mut Headless) {
    step(app.press("text:☰  All channels"));
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.press_key("backspace"));
    step(app.wait_for("text:Press Back again to leave"));
    assert!(app.screen().matches("role:alert").unwrap());
    step(app.press_key("backspace"));
    step(app.wait_for_gone("text:Press Back again to leave", Duration::from_secs(6)));

    // Switched off, the first Back leaves (nothing asks).
    step(app.press_key("right"));
    step(app.press_key("right"));
    open_menu(app);
    step(app.press(MENU_SETTINGS));
    step(app.wait_for("text:General >> nth=0"));
    step(app.press("text:General >> nth=0"));
    step(app.wait_for("text:Press Back twice to leave"));
    let switch = app
        .screen()
        .accessibility_state("role:switch[/Press Back twice to leave/]")
        .unwrap();
    assert_eq!(switch.checked, Some(Checked::True));
    assert!(!switch.selected);
    step(app.press("text:Press Back twice to leave"));
    row_value(app, "Press Back twice to leave", "Off");
    let switch = app
        .screen()
        .accessibility_state("role:switch[/Press Back twice to leave/]")
        .unwrap();
    assert_eq!(switch.checked, Some(Checked::False));
    assert!(!switch.selected);
    // A key the screen has no use for changes nothing.
    step(app.press_key("space"));
    row_value(app, "Press Back twice to leave", "Off");
    // A tap on the dimmed guide beside the panel closes the screen.
    let beside = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|n| {
            n.text.is_none()
                && n.bounds.origin.x.0 == 0.0
                && n.bounds.origin.y.0 == 0.0
                && (n.bounds.size.height.0 - 1080.0).abs() < 1.0
                && (n.bounds.size.width.0 - (1920.0 - 860.0)).abs() < 1.0
        })
        .expect("the dimmed guide beside the settings");
    step(app.press(&format!("view:{}", beside.id)));
    step(app.wait_for_gone("text:Press Back twice to leave", GONE));
    step(app.press("text:☰  All channels"));
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.press_key("backspace"));
    step(app.wait(Duration::from_millis(300)));
    assert!(!app.shows("Press Back again to leave"));
}

//! The player's keys on a TV, run headless with the TV idiom (keys stand
//! in for the remote's taps; long presses are the keymap's tests, as a
//! desktop host reports no releases): entering full screen and zapping
//! show the compact banner, which hides by itself; a custom OK binding shows the info card
//! with its action cards and one time line, OK again hides it; Right
//! reaches the cards and OK presses one; Menu (`M`) opens the shared play
//! bar and hides it again; Shift + Right (Long Right) opens history.
//!
//! Synthetic data only: a three-channel playlist "picked" from a file.

use crate::browse_fixture;

use std::time::Duration;

use crate::headless::Headless;
use nsh_core::picker::PickedFile;
use nsh_core::platform::Idiom;
use nsh_core::{ImageData, Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;

const PLAYLIST: &str = "#EXTM3U
#EXTINF:-1 tvg-id=\"one\" group-title=\"News\",Test Channel One
http://stream.example.test/one.m3u8
#EXTINF:-1 tvg-id=\"two\" group-title=\"News\",Test Channel Two
http://stream.example.test/two.m3u8
#EXTINF:-1 tvg-id=\"three\" group-title=\"Sport\",Test Channel Three
http://stream.example.test/three.m3u8
";

const IMPORT: Duration = Duration::from_secs(20);
const BANNER: &str = "id:player-banner";
const CARD: &str = "id:player-card";
const ACTIONS: &str = "id:player-actions";
const LINE: &str = "id:player-time-line";

fn count(app: &Headless, selector: &str) -> usize {
    app.screen().count(selector).unwrap()
}

#[test]
fn the_remote_shows_one_time_line_a_banner_on_zap_the_card_on_ok_and_the_strip_on_menu() {
    let Some(home) = isolated!() else { return };
    browse_fixture::two_step_preview_with(home.path(), browse_fixture::bind_ok_to_info_card);
    let config = EngineConfig {
        viewport: Size::new(px(960.0), px(540.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    let mut app = Headless::start(config).unwrap();
    app.on_pick(|_| {
        vec![PickedFile::new(
            "synthetic.m3u",
            "audio/x-mpegurl",
            ImageData::new(PLAYLIST.as_bytes().to_vec()),
        )]
    });
    let step = |r: Result<(), String>| r.unwrap_or_else(|e| panic!("{e}"));
    let gone = |app: &mut Headless, s: &str| app.wait_for_gone(s, Duration::from_secs(2));

    step(app.wait_for("id:wizard-welcome"));
    step(app.press("id:wizard-welcome-add"));
    step(app.press("id:wizard-source-type"));
    step(app.wait_for("text:Playlist file"));
    step(app.press("text:Playlist file"));
    step(app.wait_for("text:Choose a file…"));
    step(app.press("text:Choose a file…"));
    step(app.wait_for("text:synthetic.m3u"));
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", IMPORT));
    step(app.press("text:Watch"));
    step(app.wait_for("text:Test Channel One"));
    step(app.set_environment("a TV", |e| e.idiom = Idiom::Tv));

    // OK plays the focused channel in the preview, OK again goes full
    // screen: the banner, with one time line and no action cards.
    step(app.press_key("enter"));
    step(app.wait_for("text:▶"));
    step(app.press_key("enter"));
    step(app.wait_for(BANNER));
    assert_eq!(count(&app, LINE), 1, "one time line");
    assert_eq!(count(&app, CARD) + count(&app, ACTIONS), 0, "no card");

    // Down zaps: the banner again, for the next channel; it hides by
    // itself (3 s).
    step(app.press_key("down"));
    step(app.wait_for(&format!("{BANNER} >> text:/Test Channel Two$/")));
    assert_eq!(count(&app, LINE), 1);
    step(app.wait_for_gone(BANNER, Duration::from_secs(5)));

    // OK: the info card with its action cards; still one time line (the
    // card's), no separate bar. OK again hides it.
    step(app.press_key("enter"));
    step(app.wait_for(CARD));
    step(app.wait_for(ACTIONS));
    assert_eq!(count(&app, LINE), 1, "the card owns the only time line");
    assert_eq!(count(&app, BANNER), 0);
    step(app.wait_for("text:/Enter hide/"));
    step(app.press_key("enter"));
    step(gone(&mut app, CARD));
    assert_eq!(count(&app, LINE), 0);

    // Right reaches the cards (TV guide first), Right again History; OK
    // opens it. Back closes it.
    step(app.press_key("enter"));
    step(app.wait_for(ACTIONS));
    step(app.press_key("right"));
    step(app.wait_for("text:/Enter choose/"));
    step(app.press_key("right"));
    step(app.press_key("enter"));
    step(app.wait_for("text:Recently watched"));
    step(app.press_key("backspace"));
    step(gone(&mut app, "text:Recently watched"));

    // Up still zaps with the card up: the card follows the channel.
    step(app.press_key("up"));
    step(app.wait_for(&format!("{CARD} >> text:/Test Channel One$/")));

    // Menu opens the same bar as desktop/tablet, with remote focus on
    // Play/pause. Menu again hides it without changing playback.
    step(app.press_key("m"));
    step(app.wait_for("id:play-bar-layer"));
    step(gone(&mut app, CARD));
    assert_eq!(count(&app, LINE), 0, "the info card's time line is hidden");
    assert!(
        app.screen()
            .accessibility_state("id:play-bar-play-pause")
            .unwrap()
            .selected
    );
    step(app.press_key("m"));
    step(gone(&mut app, "id:play-bar-layer"));

    // Shift + Right (Long Right on a remote): the history.
    step(app.press_key("shift+right"));
    step(app.wait_for("text:Recently watched"));
    step(app.press_key("backspace"));
    step(gone(&mut app, "text:Recently watched"));

    assert!(
        !app.screen().matches("label:Mute").unwrap(),
        "the shared bar stays hidden after Menu closes it"
    );
    // Back with nothing over the video: the guide.
    step(app.press_key("backspace"));
    step(app.wait_for("text:☰  All channels"));
}

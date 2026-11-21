//! Play the last channel on start (`general.resume_last_channel`) with that
//! channel among the playlist's favorites: the guide behind full screen
//! shows Favorites, so Back lands there with the channel focused. Not a
//! favorite of this playlist, or no Favorites category offered
//! (`channels.show_favorites`): All channels, as before. One stream opens.
//! The list is decided once: the viewer's later choices and favorite
//! edits are left alone, a hidden channel focuses nothing in its place,
//! and a PIN-locked channel moves the guide only once its PIN admits it.
use crate::headless::Headless;
use crate::isolated::Home;
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_settings::{Settings, Value};
use iptv_store_sqlite::{SourceId, SourcePolicy, Store};
use std::time::Duration;

const M3U: &str = "#EXTM3U\n\
                   #EXTINF:-1 group-title=\"News\",Local A\n\
                   http://example.test/a\n\
                   #EXTINF:-1 group-title=\"News\",Local B\n\
                   http://example.test/b\n\
                   #EXTINF:-1 group-title=\"Sport\",Local C\n\
                   http://example.test/c\n";

const BANNER: &str = "id:player-banner >> text:/Local C( · Synthetic resume)?$/";

#[track_caller]
fn step(result: Result<(), String>) {
    if let Err(error) = result {
        panic!("{error}");
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Favorite {
    /// Local C, the resumed channel, is a favorite of the shown playlist.
    Here,
    /// Local C is a favorite only of another playlist with the same key
    /// (both playlists show merged, as All playlists).
    Elsewhere,
    /// Local C is a favorite of the resumed playlist, beside another one.
    HereBeside,
    /// Only Local A is a favorite.
    Not,
}

/// The stored state before the cold start.
#[derive(Clone, Copy)]
struct Setup {
    favorite: Favorite,
    offer_favorites: bool,
    /// Local C is hidden in its playlist.
    hidden: bool,
    /// Local C is protected by the parental PIN 2468.
    locked: bool,
}

impl Setup {
    fn new(favorite: Favorite) -> Self {
        Setup {
            favorite,
            offer_favorites: true,
            hidden: false,
            locked: false,
        }
    }
}

fn add_playlist(store: &mut Store, home: &Home, name: &str) -> SourceId {
    let playlist = home.path().join(format!("{name}.m3u"));
    std::fs::write(&playlist, M3U).unwrap();
    let source = store
        .add_source(&Source {
            name: name.into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new(playlist.to_string_lossy()),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    let m = iptv_providers::m3u::parse_m3u(M3U);
    store
        .replace_catalog(source, &m.groups, &m.channels.items, &[], &[])
        .unwrap();
    store
        .set_source_policy(
            source,
            SourcePolicy {
                refresh_on_start: false,
                ..Default::default()
            },
        )
        .unwrap();
    source
}

/// A stored playlist whose last channel is Local C, then the app's cold
/// start, before anything plays.
fn launch(home: &Home, setup: Setup) -> Headless {
    let dir = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&dir).unwrap();
    let mut settings = Settings::new();
    for (key, value) in [
        ("general.resume_last_channel", true),
        ("guide.autoplay", false),
        ("channels.show_favorites", setup.offer_favorites),
        ("parental.live_enabled", setup.locked),
    ] {
        settings.set(key, Value::Bool(value)).unwrap();
    }
    std::fs::write(dir.join("settings.json"), settings.to_json_string()).unwrap();
    let mut store = crate::isolated::open_database(dir.join("iptv.sqlite")).unwrap();
    let source = add_playlist(&mut store, home, "Synthetic resume");
    let m = iptv_providers::m3u::parse_m3u(M3U);
    let [a, _, c] = [0, 1, 2].map(|i| m.channels.items[i].id.clone());
    store.set_favorite(source, &a, true).unwrap();
    match setup.favorite {
        Favorite::Here => store.set_favorite(source, &c, true).unwrap(),
        Favorite::Elsewhere => {
            let other = add_playlist(&mut store, home, "Synthetic other");
            store.set_favorite(other, &c, true).unwrap();
        }
        Favorite::HereBeside => {
            store.set_favorite(source, &c, true).unwrap();
            add_playlist(&mut store, home, "Synthetic other");
        }
        Favorite::Not => {}
    }
    if setup.hidden {
        store.set_hidden(source, &c, true).unwrap();
    }
    if setup.locked {
        // Generated PIN 2468, salt 0x63, PBKDF2-SHA256/210000.
        store
            .set_parental_credential(&iptv_store_sqlite::parental::ParentalCredential {
                salt: [0x63; 16],
                verifier: [
                    108, 253, 222, 238, 40, 38, 77, 8, 100, 135, 138, 42, 133, 48, 171, 236, 123,
                    169, 134, 85, 199, 28, 212, 46, 152, 192, 13, 230, 58, 195, 156, 119,
                ],
                recovery_verifier: [0; 32],
                failures: 0,
                retry_after: 0,
            })
            .unwrap();
        store
            .set_parental_blocked(
                &iptv_core::parental::Target::live(source.0, c.as_str()),
                true,
            )
            .unwrap();
    }
    store.set_active_source(source).unwrap();
    drop(store);
    std::fs::write(dir.join(format!("last_channel-{}", source.0)), c.as_str()).unwrap();
    crate::cov_data::start()
}

/// Waits for the resumed channel full screen, its one stream opened.
fn playing(app: &mut Headless, before: u64) {
    step(app.wait_for_within(BANNER, Duration::from_secs(20)));
    step(
        app.wait_until("the resumed stream opens", Duration::from_secs(5), |_| {
            iptv_app::player_has_source(0) && iptv_app::player_opens(0) > before
        }),
    );
}

/// The banner hides on its own; Back then shows the guide.
fn to_guide(app: &mut Headless) {
    step(app.run_for(Duration::from_secs(10)));
    step(app.press_key("backspace"));
}

/// Launches, waits for the resumed channel, and goes back to the guide.
fn resume(home: &Home, setup: Setup) -> Headless {
    let before = iptv_app::player_opens(0);
    let mut app = launch(home, setup);
    playing(&mut app, before);
    to_guide(&mut app);
    assert_eq!(iptv_app::player_opens(0), before + 1, "one stream opens");
    app
}

fn channel_selected(app: &Headless, name: &str) -> bool {
    app.screen()
        .accessibility_state(&format!("id:guide-ui >> label:{name}"))
        .is_ok_and(|s| s.selected)
}

#[track_caller]
fn lands_in(app: &mut Headless, list: &str, channel: &str) {
    step(app.wait_for(&format!("text:☰  {list}")));
    assert!(channel_selected(app, channel), "{:?}", app.screen().texts());
}

/// Toggles the playing channel's favorite from the full-screen menu
/// strip, then hides it.
fn toggle_favorite(app: &mut Headless) {
    step(app.press_key("m"));
    step(app.wait_for("text:/M or Esc hide/"));
    step(app.press("text:☆ Favorite"));
    step(app.wait_for("text:★ Favorite"));
    step(app.press_key("escape"));
}

#[test]
fn a_resumed_favorite_returns_to_favorites() {
    let Some(home) = isolated!() else { return };
    let mut app = resume(&home, Setup::new(Favorite::Here));
    lands_in(&mut app, "Favorites", "Local C");
    // Favorites lists A and C only.
    assert!(
        !app.screen()
            .matches("id:guide-ui >> label:Local B")
            .unwrap()
    );
}

#[test]
fn a_resumed_channel_that_is_not_a_favorite_returns_to_all_channels() {
    let Some(home) = isolated!() else { return };
    let mut app = resume(&home, Setup::new(Favorite::Not));
    lands_in(&mut app, "All channels", "Local C");
}

#[test]
fn another_playlists_favorite_returns_to_all_channels() {
    let Some(home) = isolated!() else { return };
    let mut app = resume(&home, Setup::new(Favorite::Elsewhere));
    // The merged list's focus stays where it was, as before.
    step(app.wait_for("text:☰  All playlists"));
}

#[test]
fn a_resumed_favorite_beside_another_playlist_returns_to_favorites() {
    let Some(home) = isolated!() else { return };
    let mut app = resume(&home, Setup::new(Favorite::HereBeside));
    lands_in(&mut app, "Favorites", "Local C · Synthetic resume");
    assert!(
        !app.screen()
            .matches("id:guide-ui >> label:Local C · Synthetic other")
            .unwrap()
    );
}

#[test]
fn without_a_favorites_category_a_resumed_favorite_returns_to_all_channels() {
    let Some(home) = isolated!() else { return };
    let mut app = resume(
        &home,
        Setup {
            offer_favorites: false,
            ..Setup::new(Favorite::Here)
        },
    );
    lands_in(&mut app, "All channels", "Local C");
}

#[test]
fn favoriting_the_resumed_channel_keeps_its_list() {
    let Some(home) = isolated!() else { return };
    let before = iptv_app::player_opens(0);
    let mut app = launch(&home, Setup::new(Favorite::Not));
    playing(&mut app, before);
    toggle_favorite(&mut app);
    to_guide(&mut app);
    step(app.wait_for("text:★ Local C"));
    lands_in(&mut app, "All channels", "Local C");
    assert_eq!(iptv_app::player_opens(0), before + 1, "nothing retunes");
}

#[test]
fn a_group_chosen_over_the_resumed_channel_stays_through_a_favorite_edit() {
    let Some(home) = isolated!() else { return };
    let before = iptv_app::player_opens(0);
    let mut app = launch(&home, Setup::new(Favorite::Not));
    playing(&mut app, before);
    // The channel list over the video, then its groups: Sport.
    step(app.press_key("left"));
    step(app.wait_for("text:◂  All channels"));
    step(app.press_key("left"));
    step(app.wait_for_gone("text:◂  All channels", Duration::from_secs(3)));
    step(app.press_key("down"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for("text:◂  Sport"));
    step(app.press_key("backspace"));
    step(app.run_for(Duration::from_millis(500)));
    toggle_favorite(&mut app);
    to_guide(&mut app);
    step(app.wait_for("text:★ Local C"));
    lands_in(&mut app, "Sport", "Local C");
    assert_eq!(iptv_app::player_opens(0), before + 1, "nothing retunes");
}

#[test]
fn a_hidden_resumed_channel_focuses_nothing_in_its_place() {
    let Some(home) = isolated!() else { return };
    let before = iptv_app::player_opens(0);
    let mut app = launch(
        &home,
        Setup {
            hidden: true,
            ..Setup::new(Favorite::Here)
        },
    );
    playing(&mut app, before);
    to_guide(&mut app);
    step(app.wait_for("text:☰  All channels"));
    assert!(
        !app.screen()
            .matches("id:guide-ui >> label:Local C")
            .unwrap()
    );
    assert!(
        channel_selected(&app, "Local A"),
        "{:?}",
        app.screen().texts()
    );
}

#[test]
fn a_locked_resumed_favorite_moves_the_guide_only_after_its_pin() {
    let Some(home) = isolated!() else { return };
    let before = iptv_app::player_opens(0);
    let locked = Setup {
        locked: true,
        ..Setup::new(Favorite::Here)
    };
    let mut app = launch(&home, locked);
    step(app.wait_for_within(
        "id:parental-layer >> text:Enter parental PIN",
        Duration::from_secs(20),
    ));
    for digit in "2468".chars() {
        step(app.press_key(&digit.to_string()));
    }
    step(app.press("id:parental-layer >> text:Continue"));
    playing(&mut app, before);
    to_guide(&mut app);
    lands_in(&mut app, "Favorites", "Local C");
}

#[test]
fn a_canceled_pin_leaves_the_guide_on_all_channels() {
    let Some(home) = isolated!() else { return };
    let before = iptv_app::player_opens(0);
    let locked = Setup {
        locked: true,
        ..Setup::new(Favorite::Here)
    };
    let mut app = launch(&home, locked);
    step(app.wait_for_within(
        "id:parental-layer >> text:Enter parental PIN",
        Duration::from_secs(20),
    ));
    step(app.press("id:parental-cancel"));
    step(app.wait_for_gone(
        "id:parental-layer >> text:Enter parental PIN",
        Duration::from_secs(5),
    ));
    step(app.wait_for("text:☰  All channels"));
    assert_eq!(iptv_app::player_opens(0), before, "nothing plays");
    assert!(!iptv_app::player_has_source(0));
}

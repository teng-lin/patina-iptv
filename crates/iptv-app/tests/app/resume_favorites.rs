//! Play the last channel on start (`general.resume_last_channel`) with that
//! channel among the playlist's favorites: the guide behind full screen
//! shows Favorites, so Back lands there with the channel focused. Not a
//! favorite of this playlist, or no Favorites category offered
//! (`channels.show_favorites`): All channels, as before. One stream opens.
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
/// start; returns once the resumed channel plays full screen, with the
/// guide behind it.
fn resume(home: &Home, favorite: Favorite, offer_favorites: bool) -> Headless {
    let dir = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&dir).unwrap();
    let mut settings = Settings::new();
    for (key, value) in [
        ("general.resume_last_channel", true),
        ("guide.autoplay", false),
        ("channels.show_favorites", offer_favorites),
    ] {
        settings.set(key, Value::Bool(value)).unwrap();
    }
    std::fs::write(dir.join("settings.json"), settings.to_json_string()).unwrap();
    let mut store = crate::isolated::open_database(dir.join("iptv.sqlite")).unwrap();
    let source = add_playlist(&mut store, home, "Synthetic resume");
    let m = iptv_providers::m3u::parse_m3u(M3U);
    let [a, _, c] = [0, 1, 2].map(|i| m.channels.items[i].id.clone());
    store.set_favourite(source, &a, true).unwrap();
    match favorite {
        Favorite::Here => store.set_favourite(source, &c, true).unwrap(),
        Favorite::Elsewhere => {
            let other = add_playlist(&mut store, home, "Synthetic other");
            store.set_favourite(other, &c, true).unwrap();
        }
        Favorite::HereBeside => {
            store.set_favourite(source, &c, true).unwrap();
            add_playlist(&mut store, home, "Synthetic other");
        }
        Favorite::Not => {}
    }
    store.set_active_source(source).unwrap();
    drop(store);
    std::fs::write(dir.join(format!("last_channel-{}", source.0)), c.as_str()).unwrap();

    let before = iptv_app::player_opens(0);
    let mut app = crate::cov_data::start();
    step(app.wait_for_within(
        "id:player-banner >> text:/Local C( · Synthetic resume)?$/",
        Duration::from_secs(20),
    ));
    step(
        app.wait_until("the resumed stream opens", Duration::from_secs(5), |_| {
            iptv_app::player_has_source(0) && iptv_app::player_opens(0) > before
        }),
    );
    // The banner hides on its own; Back then shows the guide.
    step(app.run_for(Duration::from_secs(10)));
    step(app.press_key("backspace"));
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

#[test]
fn a_resumed_favorite_returns_to_favorites() {
    let Some(home) = isolated!() else { return };
    let mut app = resume(&home, Favorite::Here, true);
    lands_in(&mut app, "Favourites", "Local C");
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
    let mut app = resume(&home, Favorite::Not, true);
    lands_in(&mut app, "All channels", "Local C");
}

#[test]
fn another_playlists_favorite_returns_to_all_channels() {
    let Some(home) = isolated!() else { return };
    let mut app = resume(&home, Favorite::Elsewhere, true);
    // The merged list's focus stays where it was, as before.
    step(app.wait_for("text:☰  All playlists"));
}

#[test]
fn a_resumed_favorite_beside_another_playlist_returns_to_favorites() {
    let Some(home) = isolated!() else { return };
    let mut app = resume(&home, Favorite::HereBeside, true);
    lands_in(&mut app, "Favourites", "Local C · Synthetic resume");
    assert!(
        !app.screen()
            .matches("id:guide-ui >> label:Local C · Synthetic other")
            .unwrap()
    );
}

#[test]
fn without_a_favorites_category_a_resumed_favorite_returns_to_all_channels() {
    let Some(home) = isolated!() else { return };
    let mut app = resume(&home, Favorite::Here, false);
    lands_in(&mut app, "All channels", "Local C");
}

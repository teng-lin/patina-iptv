//! The desktop's everyday viewing workflows (#89, `docs/desktop.md`):
//! the wheel and right-click over the guide, the channel list, the poster
//! grid and recordings; context menus for guide actions and visible ⋯ controls in lists; keyboard
//! shortcuts that never reach into a text field or another window's
//! player; presentation changes (window full screen, resizing, display
//! scale) that open no stream; and one contract for closing the main
//! window while players and the recorder run.
//!
//! Synthetic data only: a generated 40-channel playlist, a generated
//! on-demand catalog on a local mock server and generated recordings.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::headless::Headless;
use crate::personalization::step;
use crate::support::Server;
use iptv_providers::{
    HttpResponse, Secret,
    model::{Source, SourceConfig, XtreamCredentials},
};
use iptv_settings::Value as Setting;
use nsh_core::{
    Command, EventKind, EventPayload, MountItem, NodeId, Point, Size, desktop::DesktopCommand,
    element::Root, platform::Idiom, prelude::*,
};
use nsh_engine::EngineConfig;
use serde_json::{Value, json};

const CHANNELS: usize = 40;
const MOVIES: usize = 48;
const RECORDINGS: usize = 36;

pub(crate) fn channel(i: usize) -> String {
    format!("Desk {i:02}")
}

/// Native player props by node, the players in the order they were
/// mounted, and window titles by window id.
#[derive(Clone, Default)]
pub(crate) struct Observed {
    props: Arc<Mutex<HashMap<NodeId, HashMap<String, Value>>>>,
    players: Arc<Mutex<Vec<NodeId>>>,
    titles: Arc<Mutex<HashMap<String, String>>>,
}

impl Observed {
    /// Surface `surface`'s player. Linux and macOS name it by its
    /// `popupSurface` routing prop (`video.rs`), which their player windows
    /// need, and the scenarios require it there.
    #[cfg(not(windows))]
    pub(crate) fn player(&self, surface: usize) -> Option<NodeId> {
        self.props
            .lock()
            .unwrap()
            .iter()
            .find(|(_, v)| v.get("popupSurface") == Some(&json!(surface)))
            .map(|(id, _)| *id)
    }

    /// Windows has no player windows and sends no routing prop: the
    /// surfaces' players are the `tv.player`s in mount order (surface 0,
    /// the persistent one, first).
    #[cfg(windows)]
    pub(crate) fn player(&self, surface: usize) -> Option<NodeId> {
        self.players.lock().unwrap().get(surface).copied()
    }

    pub(crate) fn prop(&self, surface: usize, name: &str) -> Value {
        let id = self.player(surface).expect("the player of this surface");
        self.props.lock().unwrap()[&id]
            .get(name)
            .cloned()
            .unwrap_or(Value::Null)
    }

    #[cfg_attr(windows, allow(dead_code))]
    pub(crate) fn title(&self, window: &str) -> String {
        self.titles
            .lock()
            .unwrap()
            .get(window)
            .cloned()
            .unwrap_or_default()
    }
}

/// Set to show the main window again, as opening Patina again does.
static REOPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The app, with a way to bring its hidden main window back.
#[component]
fn WindowHarness() -> impl IntoElement {
    let window = nsh_core::element::desktop::main_window();
    let _timer = set_interval(Duration::from_millis(10), move || {
        if REOPEN.swap(false, std::sync::atomic::Ordering::SeqCst) {
            window.focus();
        }
    });
    <iptv_app::App as nsh_core::element::Component>::element(())
}

/// Shows the hidden main window again and waits until it is visible.
#[cfg_attr(windows, allow(dead_code))]
fn reopen(app: &mut Headless) {
    REOPEN.store(true, std::sync::atomic::Ordering::SeqCst);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !app
        .window_state("main")
        .unwrap()
        .is_some_and(|window| window.visible)
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the main window came back"
        );
        app.settle();
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub(crate) struct Fixture {
    pub(crate) app: Headless,
    pub(crate) seen: Observed,
    _server: Server,
    // Dropped last: it stops the scenario's recorder.
    _home: crate::isolated::Home,
}

/// A desktop window on Linux with a generated Xtream playlist: 40 live
/// channels in two groups, 48 movies, and 36 scheduled recordings.
pub(crate) fn start(
    home: crate::isolated::Home,
    settings: impl FnOnce(&mut iptv_settings::Settings),
) -> Fixture {
    start_with_stream_limit(home, settings, 3)
}

/// The same synthetic fixture with an explicit account stream allowance.
pub(crate) fn start_with_stream_limit(
    home: crate::isolated::Home,
    settings: impl FnOnce(&mut iptv_settings::Settings),
    streams: i64,
) -> Fixture {
    let server = Server::start(&[(
        "/player_api.php",
        r#"{"info": {"name": "Generated Voyage"}, "episodes": {}}"#.into(),
    )]);
    let root = home.data_dir();
    std::fs::create_dir_all(&root).unwrap();
    let mut values = iptv_settings::Settings::new();
    values
        .set("general.resume_last_channel", Setting::Bool(false))
        .unwrap();
    values.set("guide.autoplay", Setting::Bool(false)).unwrap();
    settings(&mut values);
    let mut store = crate::isolated::open_database(root.join("iptv.sqlite")).unwrap();
    let source = store
        .add_source(&Source {
            name: "Generated desktop playlist".into(),
            config: SourceConfig::Xtream(XtreamCredentials {
                server_url: server.base.clone(),
                username: Secret::new("generated-user"),
                password: Secret::new("generated-pass"),
                user_agent: None,
            }),
        })
        .unwrap();
    values
        .set_in(
            "playlists.max_connections",
            &iptv_settings::Target::playlist(source.0.to_string()),
            Setting::Int(streams),
        )
        .unwrap();
    std::fs::write(root.join("settings.json"), values.to_json_string()).unwrap();
    let mut m3u = String::from("#EXTM3U\n");
    for i in 1..=CHANNELS {
        let group = if i <= CHANNELS / 2 {
            "Generated news"
        } else {
            "Generated sport"
        };
        m3u.push_str(&format!(
            "#EXTINF:-1 group-title=\"{group}\",{}\nhttp://stream.example.test/{i}.m3u8\n",
            channel(i)
        ));
    }
    let parsed = iptv_providers::m3u::parse_m3u(&m3u);
    let movies: Vec<String> = (1..=MOVIES)
        .map(|i| {
            format!(
                r#"{{"stream_id": {i}, "name": "Generated film {i:02}", "container_extension": "mp4"}}"#
            )
        })
        .collect();
    let movies = iptv_providers::xtream::parse_vod_streams(&HttpResponse::ok(format!(
        "[{}]",
        movies.join(",")
    )))
    .unwrap();
    store
        .replace_catalog(
            source,
            &parsed.groups,
            &parsed.channels.items,
            &movies.items,
            &[],
        )
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
    store.set_active_source(source).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    for i in 0..RECORDINGS {
        let start = now + 3600 * (i as i64 + 1);
        store
            .schedule_recording(
                iptv_core::recording::JobId(890 + i as i64),
                &iptv_core::recording::Request {
                    source: source.0,
                    channel: parsed.channels.items[i].id.as_str().into(),
                    channel_name: channel(i + 1),
                    title: format!("Generated recording {:02}", i + 1),
                    start,
                    end: start + 1800,
                },
                now,
            )
            .unwrap();
    }
    drop(store);

    let mut config = EngineConfig::new(Root::of::<WindowHarness>());
    config.viewport = Size::new(px(1280.0), px(720.0));
    config.environment.os = nsh_core::platform::Os::Linux;
    config.environment.idiom = Idiom::Desktop;
    let mut app = Headless::start(config).unwrap();
    let seen = Observed::default();
    let observed = seen.clone();
    let control = app.engine().control().clone();
    let windows: Arc<Mutex<HashMap<NodeId, String>>> = Arc::default();
    app.observe(move |_, batch| {
        let mut props = observed.props.lock().unwrap();
        let mut players = observed.players.lock().unwrap();
        let mut windows = windows.lock().unwrap();
        for item in &batch.items {
            match item {
                MountItem::Component { id, name } if name.as_ref() == "tv.player" => {
                    players.push(*id);
                }
                MountItem::ComponentProps { id, props: changed } => {
                    let entry = props.entry(*id).or_default();
                    for (name, value) in changed {
                        entry.insert(name.to_string(), value.clone().unwrap_or(Value::Null));
                    }
                    // Generated playback: acknowledge each load as a host would.
                    if players.contains(id)
                        && changed.iter().any(|(name, value)| {
                            name.as_ref() == "source"
                                && value.as_ref().is_some_and(|v| !v.is_null())
                        })
                    {
                        control.dispatch_event(
                            *id,
                            EventKind::NativeEvent,
                            EventPayload::Native {
                                name: "state".into(),
                                data: json!({ "state": "ready" }),
                            },
                        );
                    }
                }
                MountItem::Delete { id } => {
                    props.remove(id);
                    players.retain(|player| player != id);
                }
                MountItem::Command {
                    id,
                    command: Command::Desktop(command),
                } => {
                    let title = match &**command {
                        DesktopCommand::OpenWindow(open) => {
                            windows.insert(*id, open.options.id.clone());
                            open.options.attributes.title.clone()
                        }
                        DesktopCommand::SetAttributes(attributes) => attributes.title.clone(),
                        _ => None,
                    };
                    if let (Some(title), Some(window)) = (title, windows.get(id)) {
                        observed
                            .titles
                            .lock()
                            .unwrap()
                            .insert(window.clone(), title);
                    }
                }
                _ => {}
            }
        }
    });
    step(app.wait_for_within(
        &format!("id:guide-ui >> text:{}", channel(1)),
        Duration::from_secs(20),
    ));
    Fixture {
        app,
        seen,
        _server: server,
        _home: home,
    }
}

/// The names starting with `prefix` the screen holds now, in order. A
/// virtual list mounts the rows around its viewport only, so scrolling
/// shows as a change of names.
fn names(app: &Headless, prefix: &str) -> Vec<String> {
    let mut names: Vec<String> = app
        .screen()
        .layout_nodes()
        .into_iter()
        .filter(|n| !n.hidden)
        .filter_map(|n| {
            let text = n.text.as_ref()?.text.text.to_string();
            text.starts_with(prefix).then_some(text)
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The program and channel menu closed (it stays mounted, see-through).
#[track_caller]
fn menu_closed(app: &mut Headless) {
    step(
        app.wait_until("the menu closed", Duration::from_secs(3), |screen| {
            screen
                .view_style("id:menu-panel")
                .ok()
                .flatten()
                .is_none_or(|s| s.effective_opacity == 0.0)
        }),
    );
}

#[test]
fn the_wheel_right_click_and_hover_never_tune_and_context_menus_target_their_rows() {
    let Some(home) = isolated!() else { return };
    let mut fixture = start(home, |_| {});
    let app = &mut fixture.app;
    let opens = iptv_app::player_opens(0);

    // Hover alone never changes the channel or opens a stream.
    for i in [2, 3, 5] {
        step(app.hover(&format!("id:guide-ui >> text:{}", channel(i))));
    }
    assert_eq!(iptv_app::player_opens(0), opens, "hover tunes nothing");
    assert!(!iptv_app::player_has_source(0));

    // Either wheel over the guide tunes nothing. What each moves (the
    // channels; the time, as `guide_nav` checks with listings) is checked
    // in a real window too: the headless screen does not model the rows'
    // `sticky` place inside the timeline's sideways scroll view, and this
    // playlist has no listings to pan through (see
    // docs/validation/issue-89-desktop-journey.md).
    for delta in [(0.0, 600.0), (-900.0, 0.0), (900.0, 0.0), (0.0, -600.0)] {
        step(app.wheel("id:guide-timeline", Point::new(px(delta.0), px(delta.1))));
    }
    app.idle(Duration::from_millis(400)).unwrap();
    assert_eq!(iptv_app::player_opens(0), opens, "the wheel tunes nothing");
    assert!(!iptv_app::player_has_source(0));

    // Right-click on a channel opens its menu without tuning; Back
    // (Escape, as the real-window run checks) closes it.
    step(app.right_click(&format!("id:guide-ui >> text:{}", channel(3))));
    step(app.wait_for("id:menu-panel >> text:Add to favorites"));
    step(app.back());
    menu_closed(app);
    assert_eq!(iptv_app::player_opens(0), opens, "a menu tunes nothing");

    // The guide's groups column retains group options through right-click.
    step(app.open_guide_groups());
    step(app.wait_for("id:guide-groups >> text:Generated news"));
    step(app.press("id:guide-groups >> text:Generated news"));
    step(app.open_guide_groups());
    step(app.right_click("id:guide-groups >> text:Generated news"));
    step(app.wait_for("text:Reorder channel"));
    step(app.back());
    menu_closed(app);
    step(app.press_key("up"));
    assert_eq!(iptv_app::player_opens(0), opens);

    // Recordings: the wheel scrolls the list, each row has a ⋯ for touch
    // and a pointer, and right-click opens the same actions.
    step(app.press("id:navigation-rail >> label:Recordings"));
    step(app.wait_for("id:recording-job >> nth=1"));
    let before = names(app, "Generated recording ");
    // Wheel inside the viewport. If the list is already against one end,
    // the opposite direction should still move it.
    for delta in [3000.0, -3000.0] {
        step(app.wheel("id:recordings-screen", Point::new(px(0.0), px(delta))));
        app.idle(Duration::from_millis(400)).unwrap();
        if names(app, "Generated recording ") != before {
            break;
        }
    }
    // The list reloads once a second; a reload must not scroll it back to
    // the focused row. Let one pass before checking.
    let reload_passed = std::time::Instant::now() + Duration::from_millis(1500);
    while std::time::Instant::now() < reload_passed {
        app.idle(Duration::from_millis(200)).unwrap();
    }
    assert_ne!(
        names(app, "Generated recording "),
        before,
        "the wheel scrolls recordings"
    );
    let more = "role:button[/^More options for Generated recording/] >> nth=0";
    // After keys, moving the mouse over the list shows the ⋯ again.
    step(app.press_key("down"));
    step(app.wait_for_gone(more, Duration::from_secs(3)));
    step(app.hover("id:recording-job >> nth=0"));
    step(app.hover("id:recording-job >> nth=1"));
    step(app.wait_for(more));
    step(app.press(more));
    step(app.wait_for("text:Cancel recording >> nth=0"));
    // A scheduled job is canceled, not edited (#273).
    assert!(!app.shows("text:Edit recording"));
    step(app.back());
    let row = "id:recording-job >> nth=1";
    step(app.right_click(row));
    step(app.wait_for("text:Cancel recording >> nth=0"));
    step(app.back());
    assert_eq!(iptv_app::player_opens(0), opens);
}

#[test]
fn the_wheel_scrolls_posters_and_a_right_click_on_one_plays_nothing() {
    let Some(home) = isolated!() else { return };
    let mut fixture = start(home, |_| {});
    let app = &mut fixture.app;
    let opens = iptv_app::player_opens(0);
    step(app.press("id:navigation-rail >> label:Movies"));
    step(app.wait_for("id:vod-title >> nth=1"));
    let before = names(app, "Generated film ");
    step(app.wheel("id:vod-title >> nth=1", Point::new(px(0.0), px(2000.0))));
    app.idle(Duration::from_millis(400)).unwrap();
    assert_ne!(
        names(app, "Generated film "),
        before,
        "the wheel scrolls the poster grid"
    );
    // A sideways wheel over posters has no time to move: nothing plays.
    step(app.wheel("id:vod-title >> nth=1", Point::new(px(600.0), px(0.0))));
    step(app.right_click("id:vod-title >> nth=1"));
    app.idle(Duration::from_millis(300)).unwrap();
    assert_eq!(iptv_app::player_opens(0), opens, "nothing plays");
    assert!(!iptv_app::player_has_source(0));
}

/// The channel list's rows on screen ("12  Desk 12").
fn list_rows(app: &Headless) -> Vec<String> {
    let mut rows: Vec<String> = app
        .screen()
        .layout_nodes()
        .into_iter()
        .filter(|n| !n.hidden)
        .filter_map(|n| {
            let text = n.text.as_ref()?.text.text.to_string();
            text.contains("  Desk ").then_some(text)
        })
        .collect();
    rows.sort();
    rows.dedup();
    rows
}

/// Whether the video fills the window (the guide drawn see-through over
/// it): Patina's full screen, not the operating system's.
pub(crate) fn video_full_screen(app: &Headless) -> bool {
    app.screen()
        .view_style("id:guide-ui")
        .ok()
        .flatten()
        .is_some_and(|s| s.effective_opacity == 0.0)
}

pub(crate) fn show_controls(app: &mut Headless, control: &str) {
    step(app.hover("id:app-ui >> label:Video"));
    if !app.screen().matches(control).unwrap() {
        step(app.press("id:app-ui >> label:Video"));
    }
    step(app.wait_for(control));
}

#[test]
fn window_full_screen_resizing_and_display_scale_keep_the_stream_and_the_channel() {
    let Some(home) = isolated!() else { return };
    let mut fixture = start(home, |_| {});
    let app = &mut fixture.app;
    let seen = fixture.seen.clone();

    // Click the channel to play its preview, then click again to expand it.
    crate::browse_fixture::tap_fullscreen(app, &format!("id:guide-ui >> text:{}", channel(3)));
    assert!(video_full_screen(app), "the preview is expanded");
    let opens = iptv_app::player_opens(0);
    let player = seen.player(0).expect("the persistent player");
    let source = seen.prop(0, "source");
    assert_eq!(
        app.window_state("main").unwrap().unwrap().mode,
        nsh_core::desktop::WindowMode::Normal,
        "choosing a channel never resizes the operating system's window"
    );

    // A double click on the video: the operating system's full screen,
    // and no second play.
    step(app.press("id:app-ui >> label:Video"));
    step(app.press("id:app-ui >> label:Video"));
    assert_eq!(
        app.window_state("main").unwrap().unwrap().mode,
        nsh_core::desktop::WindowMode::FullScreen
    );

    // Live resizing, a narrow side-by-side window, display scale and
    // large text: presentation only.
    let same_stream = |app: &Headless, what: &str| {
        assert_eq!(iptv_app::player_opens(0), opens, "no open: {what}");
        assert_eq!(seen.player(0), Some(player), "same player: {what}");
        assert_eq!(seen.prop(0, "source"), source, "same stream: {what}");
        assert!(
            video_full_screen(app),
            "still full screen in the window: {what}"
        );
    };
    for (w, h) in [
        (1920.0, 1080.0),
        (480.0, 800.0),
        (400.0, 600.0),
        (1280.0, 720.0),
    ] {
        step(app.set_viewport(Size::new(px(w), px(h))));
        app.settle();
        same_stream(app, &format!("{w}x{h}"));
    }
    for ratio in [2.0, 1.25, 1.0] {
        step(app.set_environment("display scale", move |env| env.pixel_ratio = ratio));
        app.settle();
        same_stream(app, &format!("scale {ratio}"));
    }
    step(app.set_environment("large text", |env| env.font_scale = 1.6));
    app.settle();
    same_stream(app, "large text");
    step(app.set_environment("normal text", |env| env.font_scale = 1.0));

    // Back first leaves the operating system's full screen, then the
    // video; the guide is on the same channel and the stream plays on.
    step(app.back());
    assert_eq!(
        app.window_state("main").unwrap().unwrap().mode,
        nsh_core::desktop::WindowMode::Normal
    );
    same_stream(app, "leaving window full screen");
    for _ in 0..3 {
        if app.screen().matches("id:guide-ui >> text:Desk 03").unwrap() && !video_full_screen(app) {
            break;
        }
        step(app.back());
    }
    step(app.wait_for("id:guide-ui >> text:Desk 03"));
    assert!(
        app.screen()
            .accessibility_state("id:guide-ui >> label:Desk 03")
            .is_ok_and(|s| s.selected),
        "the guide comes back on the chosen channel"
    );
    assert_eq!(iptv_app::player_opens(0), opens);
    assert_eq!(seen.player(0), Some(player));

    // The channel already playing: back to full screen, no new stream.
    step(app.press(&format!("id:guide-ui >> text:{}", channel(3))));
    app.idle(Duration::from_millis(1200)).unwrap();
    assert_eq!(
        iptv_app::player_opens(0),
        opens,
        "returning plays nothing new"
    );

    // The channel list over the video: the wheel scrolls it, hover and
    // right-click change nothing, its groups column has a ⋯.
    show_controls(app, "id:app-ui >> label:Channel list");
    step(app.press("id:app-ui >> label:Channel list"));
    step(app.wait_for("id:channel-list >> text:/Desk 03/"));
    let before = list_rows(app);
    step(app.wheel(
        "id:channel-list >> text:/Desk 02/",
        Point::new(px(0.0), px(2400.0)),
    ));
    app.idle(Duration::from_millis(400)).unwrap();
    assert_ne!(list_rows(app), before, "the wheel scrolls the channel list");
    let shown = list_rows(app);
    let row = format!("id:channel-list >> text:{}", shown[shown.len() / 2]);
    step(app.hover(&row));
    step(app.right_click(&row));
    step(app.wait_for("id:menu-panel >> text:Add to favorites"));
    step(app.back());
    menu_closed(app);
    assert_eq!(iptv_app::player_opens(0), opens, "the list tunes nothing");
    step(app.press("id:channel-list >> text:/^◂/"));
    step(app.wait_for("id:channel-list-groups >> id:group-more"));
    step(app.press("id:channel-list-groups >> id:group-more"));
    step(app.wait_for("id:menu-panel >> text:Reorder channel"));
    step(app.back());
    menu_closed(app);
    assert_eq!(iptv_app::player_opens(0), opens);
    assert_eq!(seen.player(0), Some(player));
}

/// Plays `n` from the guide, then comes back to the guide with it playing
/// in the preview.
pub(crate) fn play_then_guide(app: &mut Headless, n: usize) {
    step(app.press(&format!("id:guide-ui >> text:{}", channel(n))));
    step(
        app.wait_until("the channel plays", Duration::from_secs(5), |_| {
            iptv_app::player_has_source(0)
        }),
    );
    for _ in 0..3 {
        if !video_full_screen(app) {
            break;
        }
        step(app.back());
    }
    assert!(!video_full_screen(app), "back in the guide");
}

#[test]
fn typing_shortcut_characters_in_a_field_edits_it_and_controls_no_player() {
    let Some(home) = isolated!() else { return };
    let mut fixture = start(home, |_| {});
    let app = &mut fixture.app;
    let seen = fixture.seen.clone();
    play_then_guide(app, 3);
    let opens = iptv_app::player_opens(0);
    let volume = seen.prop(0, "volume");

    step(app.press_key("mod+f"));
    step(app.wait_for("id:search-field"));
    // Every key the guide and the player bind, typed into the query.
    let typed = [
        "m", "g", "i", "=", "-", "space", "p", "n", "[", "]", "/", "f", "o", "1", "2",
    ];
    for key in typed {
        step(app.press_key(key));
    }
    let field = |app: &Headless| {
        app.screen()
            .input_text("id:search-field")
            .ok()
            .flatten()
            .unwrap_or_default()
            .to_owned()
    };
    assert_eq!(
        field(app),
        "mgi=- pn[]/fo12",
        "the field took every character"
    );
    // Backspace edits the query rather than going back; arrows move in it.
    step(app.press_key("backspace"));
    assert_eq!(field(app), "mgi=- pn[]/fo1");
    for key in ["left", "right", "home", "shift+left"] {
        step(app.press_key(key));
    }
    assert!(
        app.screen().matches("id:search-field").unwrap(),
        "still searching"
    );
    // Paste reaches the field.
    step(app.paste(
        "id:search-field",
        &nsh_core::PasteEvent::new("  synthetic paste"),
    ));
    assert!(field(app).ends_with("synthetic paste"), "{}", field(app));
    app.idle(Duration::from_millis(1200)).unwrap();
    assert_eq!(iptv_app::player_opens(0), opens, "typing zapped nothing");
    assert!(iptv_app::player_has_source(0), "typing stopped nothing");
    assert_eq!(seen.prop(0, "volume"), volume, "typing changed no sound");
    assert_eq!(
        seen.prop(0, "paused"),
        json!(false),
        "typing paused nothing"
    );
    assert!(
        app.screen().matches("id:search-screen").unwrap(),
        "typing closed nothing"
    );
    assert!(
        !video_full_screen(app),
        "f typed into the field is no full screen key"
    );
}

// Player windows ("Open in new window") exist on Linux and macOS only.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn keys_follow_the_focused_window_and_focus_never_moves_the_sound() {
    let Some(home) = isolated!() else { return };
    let mut fixture = start(home, |_| {});
    let app = &mut fixture.app;
    let seen = fixture.seen.clone();
    play_then_guide(app, 3);
    let main_opens = iptv_app::player_opens(0);

    step(app.right_click(&format!("id:guide-ui >> text:{}", channel(5))));
    step(app.wait_for("id:menu-panel >> text:Open in new window"));
    step(app.press("id:menu-panel >> text:Open in new window"));
    step(
        app.wait_until("the second player plays", Duration::from_secs(5), |_| {
            iptv_app::player_has_source(1)
        }),
    );
    assert!(app.window_state("player-window-1").unwrap().is_some());
    // The main player keeps the sound; the new window says it has none.
    app.settle();
    assert_eq!(seen.prop(0, "muted"), json!(false));
    assert_eq!(seen.prop(1, "muted"), json!(true));
    assert_eq!(seen.title("player-window-1"), "Desk 05 · muted");

    // Keys go to the window a click focused, and only to its player.
    let aux = "id:auxiliary-player-1 >> label:Video";
    step(app.hover(aux));
    step(app.press(aux));
    assert!(
        app.window_state("player-window-1")
            .unwrap()
            .unwrap()
            .focused
    );
    step(app.press_key("space"));
    app.settle();
    assert_eq!(seen.prop(1, "paused"), json!(true));
    assert_eq!(seen.prop(0, "paused"), json!(false));
    step(app.press_key("space"));
    // The main window's view keys (f, o) do nothing to it from here.
    step(app.press_key("f"));
    step(app.press_key("o"));
    assert!(
        app.screen().matches("id:guide-ui >> text:Desk 03").unwrap(),
        "the main window stayed on its guide"
    );
    assert!(app.window_state("player-window-1").unwrap().is_some());
    assert_eq!(iptv_app::player_opens(0), main_opens);
    assert_eq!(seen.prop(0, "muted"), json!(false), "focus moved no sound");
    assert_eq!(seen.prop(1, "muted"), json!(true));

    // Focusing the main window moves no sound either; its keys are its own.
    step(app.open_guide_groups());
    assert!(app.window_state("main").unwrap().unwrap().focused);
    step(app.press_key("space"));
    app.settle();
    assert_eq!(
        seen.prop(0, "paused"),
        json!(true),
        "space paused the focused window's player"
    );
    assert_eq!(seen.prop(1, "paused"), json!(false));
    step(app.press_key("space"));
    assert_eq!(seen.prop(0, "muted"), json!(false));
    assert_eq!(seen.prop(1, "muted"), json!(true));
    assert_eq!(seen.title("player-window-1"), "Desk 05 · muted");

    // Only an explicit sound control moves the sound: this window's Mute.
    step(app.hover(aux));
    if !app
        .screen()
        .matches("id:auxiliary-player-1 >> label:Mute")
        .unwrap()
    {
        step(app.press(aux));
    }
    step(app.press("id:auxiliary-player-1 >> label:Mute"));
    app.settle();
    assert_eq!(seen.prop(1, "muted"), json!(false));
    assert_eq!(
        seen.prop(0, "muted"),
        json!(true),
        "one window has the sound"
    );
    assert_eq!(seen.title("player-window-1"), "Desk 05");
    step(app.open_guide_groups());
    assert!(app.window_state("main").unwrap().unwrap().focused);
    assert_eq!(
        seen.prop(1, "muted"),
        json!(false),
        "focus kept the sound there"
    );
    assert_eq!(iptv_app::player_opens(0), main_opens);
}

// Player windows ("Open in new window") exist on Linux and macOS only.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn closing_the_main_window_during_recordings_ends_every_viewing_window() {
    let Some(home) = isolated!() else { return };
    let mut fixture = start(home, |_| {});
    let app = &mut fixture.app;
    play_then_guide(app, 3);
    let opens = iptv_app::player_opens(0);
    step(app.right_click(&format!("id:guide-ui >> text:{}", channel(6))));
    step(app.press("id:menu-panel >> text:Open in new window"));
    step(
        app.wait_until("the second player plays", Duration::from_secs(5), |_| {
            iptv_app::player_has_source(1)
        }),
    );
    assert!(app.window_state("player-window-1").unwrap().is_some());
    // The main player in its own picture-in-picture window too.
    step(app.press(&format!("id:guide-ui >> text:{}", channel(3))));
    assert_eq!(
        iptv_app::player_opens(0),
        opens,
        "the channel already plays"
    );
    show_controls(app, "id:app-ui >> label:Picture in picture");
    step(app.press("id:app-ui >> label:Picture in picture"));
    app.settle();
    assert!(app.window_state("pip").unwrap().is_some());

    // Closing keeps the app running in the background, so a recording can
    // go on: the main window hides, and every viewing window closes with it.
    step(app.press_window_close("main"));
    app.idle(Duration::from_millis(300)).unwrap();
    assert!(!app.window_state("main").unwrap().unwrap().visible);
    assert!(
        app.window_state("player-window-1").unwrap().is_none(),
        "a player window does not outlive the main window"
    );
    assert!(
        app.window_state("pip").unwrap().is_none(),
        "nor does the picture-in-picture window"
    );
    assert!(!iptv_app::player_has_source(0));
    assert!(
        fixture.seen.player(1).is_none(),
        "the second player is gone with its window"
    );

    // Opening Patina again plays nothing by itself, and choosing the same
    // channel plays it again (it no longer counts as playing).
    reopen(app);
    app.idle(Duration::from_millis(300)).unwrap();
    assert!(!iptv_app::player_has_source(0), "reopening plays nothing");
    assert!(app.window_state("pip").unwrap().is_none());
    assert!(app.window_state("player-window-1").unwrap().is_none());
    step(app.wait_for(&format!("id:guide-ui >> text:{}", channel(3))));
    let before = iptv_app::player_opens(0);
    crate::browse_fixture::tap_fullscreen(app, &format!("id:guide-ui >> text:{}", channel(3)));
    assert!(iptv_app::player_opens(0) > before);
    assert!(video_full_screen(app));

    // The same without picture-in-picture: the main player alone, back in
    // the guide's preview when the window closes.
    for _ in 0..3 {
        if !video_full_screen(app) {
            break;
        }
        step(app.back());
    }
    assert!(iptv_app::player_has_source(0));
    step(app.press_window_close("main"));
    app.idle(Duration::from_millis(300)).unwrap();
    assert!(!iptv_app::player_has_source(0));
    reopen(app);
    app.idle(Duration::from_millis(300)).unwrap();
    assert!(!iptv_app::player_has_source(0), "reopening plays nothing");
    let before = iptv_app::player_opens(0);
    step(app.press(&format!("id:guide-ui >> text:{}", channel(3))));
    step(app.wait_until(
        "the same channel plays again",
        Duration::from_secs(5),
        |_| iptv_app::player_has_source(0),
    ));
    assert!(
        iptv_app::player_opens(0) > before,
        "a new stream, not a stale one"
    );
}

/// Windows and Linux (#275): closing the main window while recordings run
/// asks whether to stop recording and quit, so no recorder runs without a
/// window. Keep recording, or Back, leaves everything as it was; Stop and
/// quit ends every recording, keeping what it holds, and quits.
#[test]
fn closing_the_main_window_during_a_capture_asks_whether_to_stop_and_quit() {
    fn here() {}
    let Some(home) = crate::isolated::run_with_env(
        std::any::type_name_of_val(&here),
        &[("IPTV_TEST_CLOSE_ASKS", "1")],
    ) else {
        return;
    };
    let database = home.data_dir().join("iptv.sqlite");
    let active = || {
        crate::isolated::open_database(&database)
            .unwrap()
            .recording_jobs()
            .unwrap()
            .iter()
            .filter(|job| job.status.active())
            .count()
    };
    let mut fixture = start(home, |_| {});
    let app = &mut fixture.app;
    play_then_guide(app, 3);
    for keep in ["id:quit-confirm-keep", "back"] {
        step(app.press_window_close("main"));
        step(app.wait_for("id:quit-confirm"));
        assert!(
            app.window_state("main").unwrap().unwrap().visible,
            "the window stays while the viewer decides"
        );
        if keep == "back" {
            step(app.back());
        } else {
            step(app.press(keep));
        }
        app.settle();
        assert!(!app.screen().matches("id:quit-confirm").unwrap());
        assert!(app.window_state("main").unwrap().unwrap().visible);
        assert!(iptv_app::player_has_source(0), "playback goes on");
        assert_eq!(active(), RECORDINGS, "the recordings go on");
    }
    step(app.press_window_close("main"));
    step(app.wait_for("id:quit-confirm"));
    assert!(app.screen().matches("id:app-ui").unwrap());
    step(app.press("id:quit-confirm-stop"));
    // The database changes before the recorder's snapshot observes the stop.
    // Wait for the window to close too, within the original quit budget.
    step(
        app.wait_until("Patina quits", Duration::from_secs(15), |screen| {
            active() == 0 && !screen.matches("id:app-ui").unwrap()
        }),
    );
    assert!(
        app.window_state("main")
            .ok()
            .flatten()
            .is_none_or(|window| !window.visible),
        "the window closed"
    );
}

#[test]
fn with_autoplay_closing_the_main_window_plays_nothing_until_the_viewer_moves() {
    let Some(home) = isolated!() else { return };
    let mut fixture = start(home, |settings| {
        settings.set("guide.autoplay", Setting::Bool(true)).unwrap();
    });
    let app = &mut fixture.app;
    play_then_guide(app, 3);
    // Closed from full screen: the guide comes back under the hidden
    // window with a live channel focused, which autoplay would tune.
    step(app.press(&format!("id:guide-ui >> text:{}", channel(3))));
    assert!(video_full_screen(app));
    let opens = iptv_app::player_opens(0);
    step(app.press_window_close("main"));
    app.idle(Duration::from_millis(2500)).unwrap();
    assert!(!app.window_state("main").unwrap().unwrap().visible);
    assert!(!iptv_app::player_has_source(0), "nothing plays hidden");
    assert_eq!(iptv_app::player_opens(0), opens, "nothing was tuned hidden");

    reopen(app);
    app.idle(Duration::from_millis(2500)).unwrap();
    assert!(!iptv_app::player_has_source(0), "nor on showing again");
    assert_eq!(iptv_app::player_opens(0), opens);

    // Moving to another channel is the viewer's own: autoplay again.
    step(app.press_key("down"));
    step(app.wait_until(
        "autoplay tunes the next channel",
        Duration::from_secs(5),
        |_| iptv_app::player_has_source(0),
    ));
}

/// How many movies the store lists under Continue watching.
fn continuing(home: &crate::isolated::Home) -> usize {
    let path = crate::isolated::data_dir(home.path()).join("iptv.sqlite");
    crate::isolated::open_database(&path)
        .unwrap()
        .vod_page(
            &iptv_core::vod::Query {
                kind: iptv_core::vod::Kind::Movie,
                continue_watching: true,
                ..Default::default()
            },
            0,
            20,
        )
        .unwrap()
        .0
        .len()
}

/// Tells surface 0 what its native player reports.
fn native(app: &mut Headless, seen: &Observed, name: &str, data: Value) {
    let player = seen.player(0).expect("the persistent player");
    app.engine().control().dispatch_event(
        player,
        EventKind::NativeEvent,
        EventPayload::Native {
            name: name.into(),
            data,
        },
    );
    app.settle();
}

/// Whether the poster `card` is selected and drawn inside the poster grid's
/// viewport: a recycled row can still be mounted outside it.
fn card_shown_and_selected(s: &nsh_testing::Screen, card: &str) -> bool {
    if !s
        .accessibility_state(card)
        .is_ok_and(|state| state.selected)
    {
        return false;
    }
    let (Ok(card), Ok(grid)) = (s.id(card), s.id("id:vod-poster-grid")) else {
        return false;
    };
    let nodes = s.layout_nodes();
    let bounds = |id| {
        nodes
            .iter()
            .find(|n| n.id == id && !n.hidden)
            .map(|n| n.bounds)
    };
    let (Some(card), Some(grid)) = (bounds(card), bounds(grid)) else {
        return false;
    };
    let top = card.origin.y.0;
    let grid_top = grid.origin.y.0;
    top >= grid_top - 0.5 && top + card.size.height.0 <= grid_top + grid.size.height.0 + 0.5
}

/// Back from a movie playing full screen, the bar hidden first if shown.
fn back_to_details(app: &mut Headless) {
    for _ in 0..2 {
        if app.screen().matches("id:vod-screen").unwrap() {
            break;
        }
        step(app.press_key("backspace"));
    }
    step(app.wait_for("text:Play from beginning"));
}

/// A movie in the operating system's full screen: Backspace first restores
/// the window, then returns to the movie's details (#147), opening
/// nothing. Chosen again from the Continue watching category and watched
/// to its end, Back twice returns to that category read again: the
/// finished movie has left it.
#[test]
fn back_from_a_movie_restores_the_window_then_its_details_and_continue_watching() {
    let Some(home) = isolated!() else { return };
    let mut fixture = start(home, |_| {});
    let seen = fixture.seen.clone();
    let app = &mut fixture.app;
    step(app.press("id:navigation-rail >> label:Movies"));
    step(app.press("label:/^Generated film 01 ·/"));
    step(app.wait_for("text:Play from beginning"));
    step(app.press("text:Play"));
    step(app.wait_for_gone("id:vod-screen", Duration::from_secs(5)));
    step(
        app.wait_until("the movie plays", Duration::from_secs(5), |_| {
            iptv_app::player_has_source(0)
        }),
    );
    let opens = iptv_app::player_opens(0);
    let player = seen.player(0);
    let token = seen.prop(0, "source")["token"].clone();
    native(
        app,
        &seen,
        "timeline",
        json!({"token": token, "seekId": 0, "ms": 37_000, "durationMs": 120_000, "seekable": true}),
    );
    step(
        app.wait_until("progress saved", Duration::from_secs(5), |_| {
            continuing(&fixture._home) == 1
        }),
    );

    // A double click: the operating system's full screen.
    step(app.press("id:app-ui >> label:Video"));
    step(app.press("id:app-ui >> label:Video"));
    assert_eq!(
        app.window_state("main").unwrap().unwrap().mode,
        nsh_core::desktop::WindowMode::FullScreen
    );
    // Backspace restores the window first; the video stays full screen in it.
    step(app.press_key("backspace"));
    assert_eq!(
        app.window_state("main").unwrap().unwrap().mode,
        nsh_core::desktop::WindowMode::Normal
    );
    assert_eq!(app.screen().count("id:vod-screen").unwrap(), 0);
    assert!(video_full_screen(app), "still full screen in the window");
    // Then Backspace returns to the movie's details.
    back_to_details(app);
    assert!(
        app.screen()
            .accessibility_state("label:Resume")
            .is_ok_and(|s| s.selected),
        "the movie's details, on Resume"
    );
    assert_eq!(iptv_app::player_opens(0), opens, "Back opens nothing");
    assert_eq!(seen.player(0), player, "the same player");

    // The Continue watching category: the movie, chosen and watched to
    // its end.
    step(app.back());
    step(app.wait_for_gone("text:Play from beginning", Duration::from_secs(5)));
    step(app.press("id:vod-continue"));
    step(app.wait_until(
        "Continue watching lists the movie",
        Duration::from_secs(5),
        |s| s.count("id:vod-title").unwrap() == 1,
    ));
    step(app.press("label:/^Generated film 01 ·/"));
    step(app.wait_for("text:Play from beginning"));
    step(app.press("text:Resume"));
    step(app.wait_for_gone("id:vod-screen", Duration::from_secs(5)));
    step(
        app.wait_until("the movie plays again", Duration::from_secs(5), |_| {
            iptv_app::player_opens(0) > opens
        }),
    );
    let opens = iptv_app::player_opens(0);
    let token = seen.prop(0, "source")["token"].clone();
    native(
        app,
        &seen,
        "timeline",
        json!({"token": token, "seekId": 0, "ms": 120_000, "durationMs": 120_000, "seekable": true}),
    );
    native(app, &seen, "state", json!({"state": "ended"}));
    step(
        app.wait_until("the end saved", Duration::from_secs(5), |_| {
            continuing(&fixture._home) == 0
        }),
    );
    back_to_details(app);
    step(app.back());
    step(app.wait_for_gone("text:Play from beginning", Duration::from_secs(5)));
    step(app.wait_until(
        "Continue watching read again: the finished movie left it",
        Duration::from_secs(5),
        |s| s.count("id:vod-title").unwrap() == 0,
    ));
    assert!(app.screen().matches("id:vod-screen").unwrap());
    assert_eq!(iptv_app::player_opens(0), opens, "Back opens nothing");
    assert_eq!(seen.player(0), player, "the same player");
}

/// Continue watching under name order holds more titles than its first
/// page: a movie chosen pages down and played, Back twice returns to that
/// category read again with the focus still on the movie, by identity
/// (#147), and opens nothing.
#[test]
fn back_from_a_movie_keeps_a_deep_continue_watching_title_focused() {
    let Some(home) = isolated!() else { return };
    let mut fixture = start(home, |_| {});
    let seen = fixture.seen.clone();
    // A second playlist's 40 films sort before the first one's 48; every
    // film is part watched, so Generated film 48 is the 88th, past the
    // first page of 80.
    let path = crate::isolated::data_dir(fixture._home.path()).join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    let extra = store
        .add_source(&Source {
            name: "Generated extra playlist".into(),
            config: SourceConfig::Xtream(XtreamCredentials {
                server_url: fixture._server.base.clone(),
                username: Secret::new("generated-extra"),
                password: Secret::new("generated-pass"),
                user_agent: None,
            }),
        })
        .unwrap();
    let films: Vec<String> = (1..=40)
        .map(|i| {
            format!(
                r#"{{"stream_id": {i}, "name": "Generated extra {i:02}", "container_extension": "mp4"}}"#
            )
        })
        .collect();
    let films = iptv_providers::xtream::parse_vod_streams(&HttpResponse::ok(format!(
        "[{}]",
        films.join(",")
    )))
    .unwrap();
    store
        .replace_catalog(extra, &[], &[], &films.items, &[])
        .unwrap();
    store
        .set_source_policy(
            extra,
            iptv_store_sqlite::SourcePolicy {
                refresh_on_start: false,
                ..Default::default()
            },
        )
        .unwrap();
    let all = iptv_core::vod::Query {
        kind: iptv_core::vod::Kind::Movie,
        ..Default::default()
    };
    let (cards, _) = store.vod_page(&all, 0, 200).unwrap();
    assert_eq!(cards.len(), MOVIES + 40);
    for card in &cards {
        store
            .save_vod_progress(
                &card.key,
                None,
                iptv_core::vod::Progress {
                    position_ms: 37_000,
                    duration_ms: Some(120_000),
                    completed: false,
                },
            )
            .unwrap();
    }
    drop(store);
    let app = &mut fixture.app;
    let deep = "label:/^Generated film 48 ·/";

    step(app.press("id:navigation-rail >> label:Movies"));
    step(app.press("id:vod-continue"));
    step(app.wait_for("text:1 / 80+"));
    step(app.press("text:Load more"));
    let total = MOVIES + 40;
    step(app.wait_for(&format!("text:1 / {total}")));
    // The keyboard walks down to the deep film, the list scrolling with it.
    step(app.press_key("right"));
    for _ in 1..total {
        step(app.press_key("down"));
    }
    step(app.wait_for(&format!("text:{total} / {total}")));
    assert!(
        app.screen()
            .accessibility_state(deep)
            .is_ok_and(|state| state.selected),
        "the deep film focused"
    );
    step(app.press_key("enter"));
    step(app.wait_for("text:Play from beginning"));
    step(app.press("text:Resume"));
    step(app.wait_for_gone("id:vod-screen", Duration::from_secs(5)));
    step(
        app.wait_until("the movie plays", Duration::from_secs(5), |_| {
            iptv_app::player_has_source(0)
        }),
    );
    let opens = iptv_app::player_opens(0);
    let player = seen.player(0);
    let token = seen.prop(0, "source")["token"].clone();
    native(
        app,
        &seen,
        "timeline",
        json!({"token": token, "seekId": 0, "ms": 50_000, "durationMs": 120_000, "seekable": true}),
    );
    back_to_details(app);
    step(app.back());
    step(app.wait_for_gone("text:Play from beginning", Duration::from_secs(5)));
    step(app.wait_until(
        "Continue watching read again, the deep movie focused",
        Duration::from_secs(5),
        |s| {
            s.matches(&format!("text:{total} / {total}")).unwrap()
                && s.matches("id:vod-focused-metadata >> text:Generated film 48")
                    .unwrap()
        },
    ));
    // The list scrolls to the film: its row is drawn and selected (#176).
    step(app.wait_until(
        "the deep film's row shown and selected",
        Duration::from_secs(5),
        |s| card_shown_and_selected(s, deep),
    ));
    assert_eq!(iptv_app::player_opens(0), opens, "Back opens nothing");
    assert_eq!(seen.player(0), player, "the same player");
    // A click on the row shown opens its details; a plain Back, with no
    // list read again, returns to it shown and selected.
    step(app.press(deep));
    step(app.wait_for("text:Play from beginning"));
    step(app.back());
    step(app.wait_for_gone("text:Play from beginning", Duration::from_secs(5)));
    step(app.wait_until(
        "the deep film's row shown and selected again",
        Duration::from_secs(5),
        |s| {
            s.matches(&format!("text:{total} / {total}")).unwrap()
                && card_shown_and_selected(s, deep)
        },
    ));
    assert_eq!(iptv_app::player_opens(0), opens, "details open nothing");
}

/// The stream surface 0's player was given last.
fn main_stream(seen: &Observed) -> String {
    if seen.player(0).is_none() {
        return String::new();
    }
    let source = seen.prop(0, "source");
    crate::support::upstream_source(&source)["url"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// #156: with autoplay on, a right-click on another channel focuses it
/// and opens its menu; dismissing the menu (Back, Backspace) tunes nothing
/// until the viewer moves the focus. A click on it still plays, and a
/// menu action that plays still does.
#[test]
fn with_autoplay_dismissing_a_right_clicked_menu_tunes_nothing() {
    let Some(home) = isolated!() else { return };
    let mut fixture = start(home, |settings| {
        settings.set("guide.autoplay", Setting::Bool(true)).unwrap();
    });
    let app = &mut fixture.app;
    let seen = fixture.seen.clone();
    // The baseline: whatever start-up played has settled.
    app.idle(Duration::from_millis(2500)).unwrap();
    let opens = iptv_app::player_opens(0);
    let before = main_stream(&seen);

    for (n, dismiss) in [(3, "Back"), (5, "Backspace")] {
        step(app.right_click(&format!("id:guide-ui >> text:{}", channel(n))));
        step(app.wait_for("id:menu-panel >> text:Add to favorites"));
        if dismiss == "Back" {
            step(app.back());
        } else {
            step(app.press_key("backspace"));
        }
        menu_closed(app);
        app.idle(Duration::from_millis(2500)).unwrap();
        assert_eq!(
            iptv_app::player_opens(0),
            opens,
            "{dismiss} on Desk {n:02}'s menu tunes nothing"
        );
        assert_eq!(main_stream(&seen), before);
    }

    // A key moves the focus on from Desk 05: autoplay as before.
    step(app.press_key("down"));
    step(app.wait_until(
        "autoplay tunes the next channel",
        Duration::from_secs(5),
        |_| main_stream(&seen).ends_with("/6.m3u8"),
    ));

    // A click on a right-clicked channel is a choice: it plays.
    step(app.right_click(&format!("id:guide-ui >> text:{}", channel(8))));
    step(app.wait_for("id:menu-panel >> text:Add to favorites"));
    step(app.back());
    menu_closed(app);
    step(app.press(&format!("id:guide-ui >> text:{}", channel(8))));
    step(
        app.wait_until("the chosen channel plays", Duration::from_secs(5), |_| {
            main_stream(&seen).ends_with("/8.m3u8")
        }),
    );

    // A menu action that plays still plays: Open in new window.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        for _ in 0..3 {
            if !video_full_screen(app) {
                break;
            }
            step(app.back());
        }
        step(app.right_click(&format!("id:guide-ui >> text:{}", channel(10))));
        step(app.wait_for("id:menu-panel >> text:Open in new window"));
        step(app.press("id:menu-panel >> text:Open in new window"));
        step(
            app.wait_until("the new window plays", Duration::from_secs(5), |_| {
                iptv_app::player_has_source(1)
            }),
        );
        assert!(main_stream(&seen).ends_with("/8.m3u8"));
    }
}

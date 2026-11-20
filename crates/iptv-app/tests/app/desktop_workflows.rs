//! The desktop's everyday viewing workflows (#89, `docs/desktop.md`):
//! the wheel and right-click over the guide, the channel list, the poster
//! grid and recordings; a visible ⋯ for every secondary action; keyboard
//! shortcuts that never reach into a text field or another window's
//! player; presentation changes (window full screen, resizing, display
//! scale) that open no stream; and one contract for closing the main
//! window while players and the recorder run.
//!
//! Synthetic data only: a generated 40-channel playlist, a generated
//! on-demand catalogue on a local mock server and generated recordings.

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

fn channel(i: usize) -> String {
    format!("Desk {i:02}")
}

/// Native player props by node, the players in the order they were
/// mounted, and window titles by window id.
#[derive(Clone, Default)]
struct Observed {
    props: Arc<Mutex<HashMap<NodeId, HashMap<String, Value>>>>,
    players: Arc<Mutex<Vec<NodeId>>>,
    titles: Arc<Mutex<HashMap<String, String>>>,
}

impl Observed {
    /// Surface `surface`'s player. Linux and macOS name it by its
    /// `popupSurface` routing prop (`video.rs`), which their player windows
    /// need; Windows has no player windows and sends no such prop, so there
    /// the surfaces' players are the `tv.player`s in mount order (surface 0,
    /// the persistent one, first).
    fn player(&self, surface: usize) -> Option<NodeId> {
        let props = self.props.lock().unwrap();
        if let Some((id, _)) = props
            .iter()
            .find(|(_, v)| v.get("popupSurface") == Some(&json!(surface)))
        {
            return Some(*id);
        }
        if props.values().any(|v| v.contains_key("popupSurface")) {
            return None;
        }
        self.players.lock().unwrap().get(surface).copied()
    }

    fn prop(&self, surface: usize, name: &str) -> Value {
        let id = self.player(surface).expect("the player of this surface");
        self.props.lock().unwrap()[&id]
            .get(name)
            .cloned()
            .unwrap_or(Value::Null)
    }

    #[cfg_attr(windows, allow(dead_code))]
    fn title(&self, window: &str) -> String {
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

struct Fixture {
    app: Headless,
    seen: Observed,
    _server: Server,
    // Dropped last: it stops the scenario's recorder.
    _home: crate::isolated::Home,
}

/// A desktop window on Linux with a generated Xtream playlist: 40 live
/// channels in two groups, 48 movies, and 36 scheduled recordings.
fn start(
    home: crate::isolated::Home,
    settings: impl FnOnce(&mut iptv_settings::Settings),
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
            Setting::Int(3),
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

/// The programme and channel menu closed (it stays mounted, see-through).
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
fn the_wheel_right_click_and_hover_never_tune_and_every_menu_has_a_visible_way_in() {
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
    step(app.wait_for("id:menu-panel >> text:Add to favourites"));
    step(app.back());
    menu_closed(app);
    assert_eq!(iptv_app::player_opens(0), opens, "a menu tunes nothing");

    // The guide's groups column: right-click and the shown group's ⋯ open
    // the same group options.
    step(app.press("text:/^☰  /"));
    step(app.wait_for("id:guide-groups >> text:Generated news"));
    step(app.press("id:guide-groups >> text:Generated news"));
    // A click shows the group and closes the column; the shown group's
    // row has the ⋯ when it opens again.
    step(app.press("text:/^☰  /"));
    step(app.wait_for("id:guide-groups >> id:group-more"));
    assert_eq!(
        app.screen()
            .count("id:guide-groups >> id:group-more")
            .unwrap(),
        1,
        "only the shown group's row has a ⋯"
    );
    step(app.press("id:guide-groups >> id:group-more"));
    step(app.wait_for("text:Reorder channels"));
    step(app.back());
    menu_closed(app);
    step(app.right_click("id:guide-groups >> text:Generated news"));
    step(app.wait_for("text:Reorder channels"));
    step(app.back());
    menu_closed(app);
    // Keys hide the ⋯: they have Menu and Shift+F10.
    step(app.press_key("down"));
    step(app.wait_for_gone("id:group-more", Duration::from_secs(3)));
    // Moving the mouse again brings it back, without moving the focus or
    // tuning anything.
    let focused = |app: &Headless| {
        app.screen()
            .accessibility_state("id:guide-groups >> label:Generated sport")
            .is_ok_and(|s| s.selected)
    };
    let before = focused(app);
    step(app.hover(&format!("id:guide-ui >> text:{}", channel(24))));
    step(app.hover(&format!("id:guide-ui >> text:{}", channel(26))));
    step(app.wait_for("id:guide-groups >> id:group-more"));
    assert_eq!(focused(app), before, "the mouse moved no focus");
    step(app.press_key("up"));
    assert_eq!(iptv_app::player_opens(0), opens);

    // Recordings: the wheel scrolls the list, each row has a ⋯ for touch
    // and a pointer, and right-click opens the same actions.
    step(app.press("id:navigation-rail >> label:Recordings"));
    step(app.wait_for("id:recording-job >> nth=1"));
    let before = names(app, "Generated recording ");
    step(app.wheel("id:recording-job >> nth=1", Point::new(px(0.0), px(3000.0))));
    app.idle(Duration::from_millis(400)).unwrap();
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
    step(app.wait_for("text:Edit recording >> nth=0"));
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
fn video_full_screen(app: &Headless) -> bool {
    app.screen()
        .view_style("id:guide-ui")
        .ok()
        .flatten()
        .is_some_and(|s| s.effective_opacity == 0.0)
}

fn show_controls(app: &mut Headless, control: &str) {
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

    // A click on a channel plays it full screen inside the window.
    step(app.press(&format!("id:guide-ui >> text:{}", channel(3))));
    step(
        app.wait_until("Desk 03 plays", Duration::from_secs(5), |_| {
            iptv_app::player_has_source(0)
        }),
    );
    assert!(video_full_screen(app), "choosing is watching");
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
    step(app.wait_for("id:menu-panel >> text:Add to favourites"));
    step(app.back());
    menu_closed(app);
    assert_eq!(iptv_app::player_opens(0), opens, "the list tunes nothing");
    step(app.press("id:channel-list >> text:/^◂/"));
    step(app.wait_for("id:channel-list-groups >> id:group-more"));
    step(app.press("id:channel-list-groups >> id:group-more"));
    step(app.wait_for("id:menu-panel >> text:Reorder channels"));
    step(app.back());
    menu_closed(app);
    assert_eq!(iptv_app::player_opens(0), opens);
    assert_eq!(seen.player(0), Some(player));
}

/// Plays `n` from the guide, then comes back to the guide with it playing
/// in the preview.
fn play_then_guide(app: &mut Headless, n: usize) {
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

// Player windows ("Watch in new window") exist on Linux and macOS only.
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
    step(app.wait_for("id:menu-panel >> text:Watch in new window"));
    step(app.press("id:menu-panel >> text:Watch in new window"));
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
    step(app.press("text:/^☰  /"));
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
    step(app.press("text:/^☰  /"));
    assert!(app.window_state("main").unwrap().unwrap().focused);
    assert_eq!(
        seen.prop(1, "muted"),
        json!(false),
        "focus kept the sound there"
    );
    assert_eq!(iptv_app::player_opens(0), main_opens);
}

// Player windows ("Watch in new window") exist on Linux and macOS only.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn closing_the_main_window_during_recordings_ends_every_viewing_window() {
    let Some(home) = isolated!() else { return };
    let mut fixture = start(home, |_| {});
    let app = &mut fixture.app;
    play_then_guide(app, 3);
    let opens = iptv_app::player_opens(0);
    step(app.right_click(&format!("id:guide-ui >> text:{}", channel(6))));
    step(app.press("id:menu-panel >> text:Watch in new window"));
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

    // Scheduled recordings keep the app running in the background: the
    // main window hides, and every viewing window closes with it.
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
    step(app.press(&format!("id:guide-ui >> text:{}", channel(3))));
    step(app.wait_until(
        "the same channel plays again",
        Duration::from_secs(5),
        |_| iptv_app::player_has_source(0),
    ));
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

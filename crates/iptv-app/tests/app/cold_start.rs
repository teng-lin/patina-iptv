//! A cold start with a stored playlist, headless: the guide's shell comes
//! first (never the synthetic guide), the last channel opens once, before
//! the guide is read, and stays open when the guide arrives (its channel
//! found by key, not opened again); the first screen's programs, then the
//! whole stored window, reach the guide.
//!
//! Synthetic data only.

use std::{
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::headless::Headless;
use iptv_epg::format_xmltv_time;
use iptv_store_sqlite::ChannelQuery;
use nsh_core::picker::PickedFile;
use nsh_core::{
    ImageData, Size,
    element::{Component, Root, Scope},
    prelude::*,
};
use nsh_engine::EngineConfig;

const IMPORT: Duration = Duration::from_secs(20);

// Installed by the test root before bootstrap's asynchronous continuation runs.
static PREPARATION: Mutex<Option<iptv_runtime::TestGate>> = Mutex::new(None);
static OPEN_SESSIONS: OpenSessions = OpenSessions(Mutex::new(Vec::new()));

struct OpenSessions(Mutex<Vec<String>>);
impl log::Log for OpenSessions {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }
    fn log(&self, record: &log::Record) {
        let line = record.args().to_string();
        if line.starts_with("iptv player 0 open: session ") {
            self.0.lock().unwrap().push(line);
        }
    }
    fn flush(&self) {}
}

struct HeldEarlyApp;
impl Component for HeldEarlyApp {
    type Props = <iptv_app::App as Component>::Props;
    type Event = <iptv_app::App as Component>::Event;

    fn view(props: Self::Props, cx: Scope<Self::Event>) -> impl IntoElement {
        let app = iptv_app::App::view(props, cx);
        *PREPARATION.lock().unwrap() = Some(iptv_app::hold_source_preparation());
        app
    }
}

/// Hour-long programs from a day back to two days ahead: "Now Show" on
/// air now, "Late Show" ten hours on (past the first screen).
fn guide(now: i64) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\"?><tv>\
         <channel id=\"one\"><display-name>One</display-name></channel>\
         <channel id=\"two\"><display-name>Two</display-name></channel>",
    );
    let hour = now - now.rem_euclid(3600);
    for channel in ["one", "two"] {
        let mut t = hour - 86_400;
        while t < hour + 2 * 86_400 {
            let title = match t - hour {
                0 => "Now Show",
                36_000 => "Late Show",
                _ => "Other Show",
            };
            xml.push_str(&format!(
                "<programme channel=\"{channel}\" start=\"{}\" stop=\"{}\"><title>{title}</title></programme>",
                format_xmltv_time(t),
                format_xmltv_time(t + 3600)
            ));
            t += 3600;
        }
    }
    xml + "</tv>"
}

fn app() -> Headless {
    let config = EngineConfig {
        viewport: Size::new(px(1280.0), px(720.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    Headless::start(config).unwrap()
}

#[test]
fn the_last_channel_opens_once_before_the_guide() {
    let Some(home) = isolated!() else { return };
    cold_start(home, false);
}

#[test]
fn the_guide_adopts_the_original_early_preparation_before_native_publication() {
    let Some(home) = isolated!() else { return };
    log::set_logger(&OPEN_SESSIONS).expect("this process's only logger");
    log::set_max_level(log::LevelFilter::Info);
    cold_start(home, true);
}

fn cold_start(home: crate::isolated::Home, hold_preparation: bool) {
    crate::performance::prepare(&home);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let guide_path = home.path().join("guide.xml");
    std::fs::write(&guide_path, guide(now)).unwrap();
    let playlist = format!(
        "#EXTM3U url-tvg=\"{}\"\n\
         #EXTINF:-1 tvg-id=\"one\" group-title=\"News\",Test Channel One\nhttp://stream.example.test/one.m3u8\n\
         #EXTINF:-1 tvg-id=\"two\" group-title=\"News\",Test Channel Two\nhttp://stream.example.test/two.m3u8\n",
        guide_path.display()
    );
    let step = |r: Result<(), String>| r.unwrap_or_else(|e| panic!("{e}"));

    // First run: the playlist is added; nothing plays.
    let mut first = app();
    first.answer_picks_myself();
    step(first.wait_for("id:wizard-welcome"));
    step(first.press("id:wizard-welcome-add"));
    step(first.press("id:wizard-source-type"));
    step(first.wait_for("text:Playlist file"));
    step(first.press("text:Playlist file"));
    step(first.wait_for("text:Choose a file…"));
    // The form focuses its file field by identity. Activate that field through
    // the remote key path, without depending on a newly mounted row's layout.
    step(first.press_key("enter"));
    // The pick request may arrive after press_key's quiet period on a busy
    // runner. Answer the request explicitly once it exists: wait_for only
    // pumps batches and does not run Headless's automatic picker handler.
    step(first.wait_until(
        "the playlist file picker",
        Duration::from_secs(5),
        |screen| !screen.picks().is_empty(),
    ));
    assert_eq!(first.picks().len(), 1, "one activation opens one picker");
    step(first.answer_pick(vec![PickedFile::new(
        "synthetic.m3u",
        "audio/x-mpegurl",
        ImageData::new(playlist.as_bytes().to_vec()),
    )]));
    step(first.wait_for("text:synthetic.m3u"));
    step(first.press("text:Add playlist"));
    step(first.wait_for_within("text:Playlist added", IMPORT));
    step(first.press("text:Watch"));
    step(first.wait_for_within("text:Now Show", IMPORT));
    drop(first);
    if std::env::var_os("IPTV_PERF_SAMPLE").is_some() {
        let mut settings = iptv_settings::Settings::new();
        settings
            .set("epg.update", iptv_settings::Value::choice("on_request"))
            .unwrap();
        std::fs::write(
            home.data_dir().join("settings.json"),
            settings.to_json_string(),
        )
        .unwrap();
    }

    // Channel two played last.
    let data = home.data_dir();
    let mut store = crate::isolated::open_database(data.join("iptv.sqlite")).unwrap();
    let source = store.sources().unwrap()[0].id;
    let two = store
        .load_channels(source, &ChannelQuery::default())
        .unwrap()
        .into_iter()
        .find(|row| row.name == "Test Channel Two")
        .unwrap();
    if std::env::var_os("IPTV_PERF_SAMPLE").is_some() {
        store
            .set_source_policy(
                source,
                iptv_store_sqlite::SourcePolicy {
                    refresh_on_start: false,
                    ..Default::default()
                },
            )
            .unwrap();
    }
    drop(store);
    std::fs::write(
        data.join(format!("last_channel-{}", source.0)),
        two.channel.id.as_str(),
    )
    .unwrap();

    // Cold start: the shell, never the synthetic guide; channel two opens
    // at once, before the guide is read.
    let opens = iptv_app::player_opens(0);
    let journey = crate::performance::Sample::start("cached-guide", 0);
    OPEN_SESSIONS.0.lock().unwrap().clear();
    let mut app = if hold_preparation {
        Headless::start(EngineConfig {
            viewport: Size::new(px(1280.0), px(720.0)),
            ..EngineConfig::new(Root::of::<HeldEarlyApp>())
        })
        .unwrap()
    } else {
        app()
    };
    if hold_preparation {
        step(
            app.wait_until("test root installed the preparation gate", IMPORT, |_| {
                PREPARATION.lock().unwrap().is_some()
            }),
        );
    }
    let preparation = hold_preparation.then(|| PREPARATION.lock().unwrap().take().unwrap());
    if let Some(preparation) = &preparation {
        step(crate::isolated::wait_outside(
            &mut app,
            "early preparation entered",
            IMPORT,
            || preparation.wait_entered(Duration::ZERO),
        ));
        // Program cells only arrive with the catalog/guide, so this passes
        // the actual adoption boundary while the first request is still held.
        step(app.wait_for_within("text:Now Show", IMPORT));
        step(app.idle(Duration::from_millis(500)).map(|_| ()));
        assert_eq!(
            iptv_app::player_opens(0),
            opens,
            "preparation remains unpublished"
        );
        assert!(!iptv_app::player_has_source(0));
        preparation.release();
    }
    assert!(!app.shows("text:Channel 1"), "the synthetic guide showed");
    step(app.wait_for_within("text:Test Channel Two", IMPORT));
    step(
        app.wait_until("the last channel source is published", IMPORT, |_| {
            iptv_app::player_has_source(0) && iptv_app::player_opens(0) > opens
        }),
    );
    assert_eq!(iptv_app::player_opens(0), opens + 1, "opened once");
    if hold_preparation {
        let sessions = OPEN_SESSIONS.0.lock().unwrap();
        assert_eq!(sessions.len(), 1, "one native publication");
        assert!(
            sessions[0].starts_with("iptv player 0 open: session 1,"),
            "adoption must keep the original health request: {sessions:?}"
        );
    }
    // The guide arrives (first screen, then the whole window) and the
    // playing channel is found in it: back to the guide shows it, with
    // programs past the first screen. Its first screen is on the tree
    // under the video once "Now Show" is; the whole window follows from
    // the store without a text of its own, so the app is left to finish
    // rendering before the count is read.
    step(app.wait_for_within("text:Now Show", IMPORT));
    step(app.idle(Duration::from_millis(500)).map(|_| ()));
    journey.finish(
        "guide-mounted",
        0,
        iptv_app::store_reads(),
        iptv_app::player_opens(0) - opens,
    );
    step(app.back());
    step(app.wait_for_within("text:Now Show", IMPORT));
    assert_eq!(
        iptv_app::player_opens(0),
        opens + 1,
        "the guide's arrival opened it again"
    );
}

#[test]
fn a_newer_database_shows_recovery_details_instead_of_the_first_run_wizard() {
    let Some(home) = isolated!() else { return };
    let dir = home.data_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("iptv.sqlite");
    let supported = iptv_store_sqlite::SCHEMA_VERSION;
    let found = supported + 1;
    iptv_store_sqlite::Store::open_in_memory()
        .unwrap()
        .snapshot_to(&path)
        .unwrap();
    let database = rusqlite::Connection::open(&path).unwrap();
    database
        .execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, 'future', 0)",
            [found],
        )
        .unwrap();
    drop(database);

    let mut app = app();
    app.wait_for("id:store-startup-failure").unwrap();
    app.wait_for("text:Update Patina to open your library")
        .unwrap();
    let handed = app
        .back()
        .expect_err("startup recovery without details leaves Back to the platform");
    assert!(handed.contains("nothing to go back from"), "{handed}");
    assert!(!app.screen().matches("id:store-recovery-details").unwrap());
    assert!(!app.screen().matches("id:store-recovery-retry").unwrap());
    app.press_key("enter").unwrap();
    app.wait_for(&format!(
        "text:Database schema version {found} is newer than this build supports ({supported})."
    ))
    .unwrap();
    assert!(app.screen().shows(&path.to_string_lossy()));
    app.wait_for(
        "text:Install the same version you last used, or a newer one, then reopen Patina.",
    )
    .unwrap();
    app.back().unwrap();
    assert!(!app.screen().matches("id:store-recovery-details").unwrap());
    let handed = app
        .back()
        .expect_err("after closing recovery details, Back is the platform's again");
    assert!(handed.contains("nothing to go back from"), "{handed}");
    app.press_remote(nsh_tv::RemoteKey::Menu).unwrap();
    app.wait(Duration::from_secs(3)).unwrap();
    let screen = app.screen();
    assert!(screen.matches("id:store-startup-failure").unwrap());
    assert!(!screen.shows("Set up your library"));
    assert!(!screen.matches("id:wizard-source-picker").unwrap());
}

#![allow(dead_code)]
use crate::headless::Headless;
use crate::isolated::Home;
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_settings::Settings;
use iptv_store_sqlite::{SourceId, SourcePolicy};
use nsh_core::{Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};
pub const M3U: &str = "#EXTM3U\n\
                       #EXTINF:-1 group-title=\"News\" tvg-logo=\"http://example.test/logo.svg\",Local A\n\
                       http://example.test/a\n\
                       #EXTINF:-1 group-title=\"News\",Local B\n\
                       http://example.test/b\n\
                       #EXTINF:-1 group-title=\"Sport\",Local C\n\
                       http://example.test/c\n";
pub struct Fixture {
    // Fields drop in declaration order: finish the engine before Home stops
    // the retained native recorder and removes the scenario's storage.
    pub app: Headless,
    pub path: PathBuf,
    pub source: SourceId,
    pub home: Home,
}
#[track_caller]
pub fn step(r: Result<(), String>) {
    if let Err(error) = r {
        panic!("{error}");
    }
}
/// UI changes are optimistic; observe the settings file's atomic publication
/// separately when a scenario checks persistence.
#[track_caller]
pub fn saved_settings(
    app: &mut Headless,
    path: &Path,
    expected: impl Fn(&Settings) -> bool,
) -> Settings {
    let deadline = Instant::now() + crate::isolated::scaled_duration(Duration::from_secs(10));
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok((settings, report)) = Settings::from_json(&text)
            && report.is_clean()
            && expected(&settings)
        {
            return settings;
        }
        assert!(
            Instant::now() < deadline,
            "settings did not reach the expected durable state"
        );
        step(app.wait(Duration::from_millis(10)));
    }
}
/// `home`: the scenario's own (`isolated!()`).
pub fn start(home: Home, settings: Settings, manual: bool) -> Fixture {
    start_with_setup(home, settings, manual, |_| {})
}

/// Install observation before the app's initial batches are applied.
pub fn start_with_setup(
    home: Home,
    settings: Settings,
    manual: bool,
    setup: impl FnOnce(&mut Headless),
) -> Fixture {
    let mut fixture = launch(home, settings, manual, |_| {}, setup);
    let app = &mut fixture.app;
    step(app.press("text:/^☰  /"));
    step(app.press_key("right"));
    fixture
}

/// The app on the host `configure` describes, as it first shows the guide:
/// no press or key has reached it yet.
pub fn launch(
    home: Home,
    settings: Settings,
    manual: bool,
    configure: impl FnOnce(&mut EngineConfig),
    setup: impl FnOnce(&mut Headless),
) -> Fixture {
    launch_prepared(home, settings, manual, |_, _| {}, configure, setup)
}

/// [`launch`], with `prepare` changing the stored library (its database
/// path and the source) before the app starts.
pub fn launch_prepared(
    home: Home,
    settings: Settings,
    manual: bool,
    prepare: impl FnOnce(&Path, SourceId),
    configure: impl FnOnce(&mut EngineConfig),
    setup: impl FnOnce(&mut Headless),
) -> Fixture {
    let dir = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("settings.json"), settings.to_json_string()).unwrap();
    let playlist = home.path().join("synthetic.m3u");
    std::fs::write(&playlist, M3U).unwrap();
    let path = dir.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
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
    for c in &m.channels.items {
        store.set_favorite(source, &c.id, true).unwrap();
    }
    drop(store);
    prepare(&path, source);
    let mut config = EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    if manual {
        config.clock = nsh_core::time::Clock::manual(SystemTime::now());
    }
    configure(&mut config);
    let mut app = Headless::start(config).unwrap();
    setup(&mut app);
    step(app.wait_for_within("text:★ Local A", Duration::from_secs(20)));
    Fixture {
        home,
        path,
        source,
        app,
    }
}
#[track_caller]
pub fn menu(app: &mut Headless, label: &str) {
    step(app.press_key("m"));
    step(app.wait_for("text:Manage favourites…"));
    step(app.press(&format!("text:{label}")));
}
#[track_caller]
pub fn group(app: &mut Headless, name: &str) {
    // The list-name button is the only text beginning with this mark.
    step(app.press("text:/^☰  /"));
    step(app.wait_for(&format!("id:guide-groups >> text:{name}")));
    step(app.press(&format!("id:guide-groups >> text:{name}")));
    step(app.press_key("right"));
}
#[track_caller]
pub fn new_group(app: &mut Headless, name: &str) {
    step(app.press("text:New group"));
    step(app.type_text("text:Enter text", name));
    step(app.submit(&format!("text:{name}")));
    // Submission queues a store write. A following press must wait for its
    // acknowledgement instead of hitting the still-open modal editor.
    step(app.wait_for_gone("text:Group name", Duration::from_secs(5)));
    step(app.wait_for(&format!("text:{name} · 0 channels")));
}

/// Debounce uses the manual UI clock; authorization completes on native workers.
/// Observe the load acknowledgement without advancing the debounce deadline.
#[track_caller]
pub fn loaded_stream(
    app: &mut Headless,
    opened: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    url: &str,
) {
    step(app.wait_until(
        "authorized synthetic stream",
        Duration::from_secs(5),
        |_| {
            opened
                .lock()
                .unwrap()
                .last()
                .is_some_and(|last| last == url)
        },
    ));
}

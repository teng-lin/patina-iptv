//! Generated catch-up choices use one policy through ordinary guide inputs.
use crate::{
    headless::Headless,
    personalization::{self, Fixture, step},
    support::Server,
};
use iptv_settings::{Settings, Value};
use nsh_core::{MountItem, platform::Idiom};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

const WAIT: Duration = Duration::from_secs(10);
type Sources = Arc<Mutex<Vec<String>>>;

struct Streams {
    _server: Server,
    _requests: Sources,
}

#[cfg(target_os = "linux")]
impl Streams {
    fn requests(&self) -> Vec<String> {
        self._requests.lock().unwrap().clone()
    }
}

fn start(home: crate::isolated::Home) -> (Fixture, Streams, Sources, i64) {
    let now = crate::support::now().div_euclid(60) * 60;
    let requests: Sources = Arc::default();
    let received = requests.clone();
    let server = Server::start_with(
        &[
            ("/one.ts", "Generated media one".into()),
            ("/two.ts", "Generated media two".into()),
        ],
        move |target| received.lock().unwrap().push(target.into()),
    );
    let guide = home.path().join("generated.xml");
    let mut xml = String::from("<tv>");
    for (id, suffix) in [("one", "A"), ("two", "B")] {
        xml.push_str(&format!(
            "<channel id=\"{id}\"><display-name>Local {suffix}</display-name></channel>"
        ));
        for slot in -2..=1 {
            let start = now - 1200 + i64::from(slot) * 3600;
            xml.push_str(&format!("<programme channel=\"{id}\" start=\"{}\" stop=\"{}\"><title>Generated {suffix} {slot}</title></programme>", iptv_epg::format_xmltv_time(start), iptv_epg::format_xmltv_time(start + 3600)));
        }
    }
    std::fs::write(&guide, xml + "</tv>").unwrap();
    let playlist = format!(
        "#EXTM3U\n#EXTINF:-1 tvg-id=\"one\" group-title=\"News\" catchup=\"shift\" catchup-days=\"3\",Local A\n{}\n#EXTINF:-1 tvg-id=\"two\" group-title=\"Sport\" catchup=\"shift\" catchup-days=\"3\",Local B\n{}\n",
        server.url("/one.ts"),
        server.url("/two.ts")
    );
    let mut settings = Settings::new();
    for (key, value) in [
        ("general.resume_last_channel", Value::Bool(false)),
        ("guide.autoplay", Value::Bool(false)),
        ("guide.stay_on_guide", Value::Bool(true)),
        ("epg.update", Value::choice("on_request")),
        ("parental.unlock_window", Value::choice("every_time")),
    ] {
        settings.set(key, value).unwrap();
    }
    let sources: Sources = Arc::default();
    let observed = sources.clone();
    let mut fixture = personalization::launch_playlist(
        home,
        settings,
        false,
        &playlist,
        |path, source| {
            let mut store = crate::isolated::open_database(path).unwrap();
            store
                .set_guide_sources(
                    source,
                    &[iptv_store_sqlite::GuideSource {
                        feed: iptv_epg::FeedId(355),
                        provider_index: None,
                        url: iptv_providers::SecretUrl::new(guide.to_string_lossy()),
                        name: Some("Generated archive guide".into()),
                        enabled: true,
                        shift_minutes: 0,
                    }],
                )
                .unwrap();
            let group = store
                .groups(source, None)
                .unwrap()
                .into_iter()
                .find(|group| group.name == "News")
                .unwrap();
            store
                .set_guide_group_preference(
                    source,
                    group.id.as_str(),
                    iptv_core::organization::GroupPreference::ExternalPlayer(true),
                )
                .unwrap();
        },
        |config| config.environment.idiom = Idiom::Tv,
        move |app| {
            app.observe(move |_, batch| {
                for item in &batch.items {
                    if let MountItem::ComponentProps { props, .. } = item {
                        for (name, value) in props {
                            if name.as_ref() == "source"
                                && let Some(source) = value
                            {
                                let source = crate::support::upstream_source(source);
                                if let Some(url) = source["url"].as_str() {
                                    observed.lock().unwrap().push(url.into());
                                }
                            }
                        }
                    }
                }
            })
        },
    );
    step(fixture.app.wait_for_within("text:Generated A 0", WAIT));
    personalization::group(&mut fixture.app, "News");
    (
        fixture,
        Streams {
            _server: server,
            _requests: requests,
        },
        sources,
        now - 4800,
    )
}

fn previous(app: &mut Headless) {
    step(app.press_key("home"));
    step(app.press_key("left"));
    step(app.wait_for("text:Generated A -1"));
}

fn menu_replay(app: &mut Headless) {
    step(app.press_key("m"));
    step(app.wait_for("id:menu-panel >> text:Play from the beginning"));
    step(app.press("id:menu-panel >> text:Play from the beginning"));
}

fn exact_archive(url: &str, start: i64) {
    assert!(url.contains("/one.ts?"), "channel one's archive: {url}");
    assert!(
        url.split('?')
            .nth(1)
            .unwrap()
            .split('&')
            .any(|pair| pair == format!("utc={start}")),
        "captured occurrence: {url}"
    );
}

#[test]
fn both_archive_entries_fall_back_once_without_an_external_handler() {
    let Some(home) = isolated!() else { return };
    iptv_app::set_external_player_handler(Some(false));
    let (mut fixture, _server, sources, at) = start(home);
    let app = &mut fixture.app;
    for menu in [false, true] {
        previous(app);
        let before = iptv_app::player_opens(0);
        let published = sources.lock().unwrap().len();
        if menu {
            menu_replay(app);
        } else {
            step(app.press_key("enter"));
        }
        step(app.wait_until("one internal archive open", WAIT, |_| {
            iptv_app::player_opens(0) == before + 1 && sources.lock().unwrap().len() > published
        }));
        exact_archive(sources.lock().unwrap().last().unwrap(), at);
        assert_eq!(iptv_app::player_opens(0), before + 1);
        // The missing-handler reset offer can stay over the guide. Dismiss it
        // before returning from full screen, without changing the preference.
        if app.shows("id:menu-panel") {
            step(app.back());
        }
        if app.shows("id:play-bar-layer") {
            step(app.press("id:play-bar-back"));
        } else {
            step(app.back());
        }
    }
}

#[test]
fn a_menu_archive_waits_for_parental_pin_and_cancel_opens_nothing() {
    let Some(home) = isolated!() else { return };
    iptv_app::set_external_player_handler(Some(false));
    let (mut fixture, _server, sources, at) = start(home);
    let app = &mut fixture.app;
    personalization::menu(app, "Protect channel with PIN");
    let pin = |app: &mut Headless| {
        for digit in ["2", "4", "6", "8"] {
            step(app.press_key(digit));
        }
        step(app.press("id:parental-layer >> text:Continue"));
    };
    step(app.wait_for("id:parental-layer >> text:Choose a new PIN"));
    pin(app);
    step(app.wait_for("id:parental-layer >> text:Confirm new PIN"));
    pin(app);
    step(app.wait_for("text:Save your recovery code"));
    step(app.press("text:I saved the code"));
    personalization::menu(app, "Manage PIN-protected content…");
    step(app.wait_for("text:Blocked content"));
    step(app.press("text:Lock now"));
    previous(app);
    let before = iptv_app::player_opens(0);
    menu_replay(app);
    step(app.wait_for("id:parental-layer >> text:Enter parental PIN"));
    assert_eq!(iptv_app::player_opens(0), before);
    step(app.press("id:parental-cancel"));
    assert_eq!(iptv_app::player_opens(0), before);
    menu_replay(app);
    step(app.wait_for("id:parental-layer >> text:Enter parental PIN"));
    pin(app);
    step(
        app.wait_until("authorized captured archive published", WAIT, |_| {
            iptv_app::player_opens(0) == before + 1 && !sources.lock().unwrap().is_empty()
        }),
    );
    exact_archive(sources.lock().unwrap().last().unwrap(), at);
    assert_eq!(iptv_app::player_opens(0), before + 1);
}

#[cfg(target_os = "linux")]
struct Player {
    alive: std::path::PathBuf,
    release: std::path::PathBuf,
    log: std::path::PathBuf,
}

#[cfg(target_os = "linux")]
impl Player {
    fn start(path: &std::path::Path) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let bin = path.join("generated-player");
        std::fs::create_dir(&bin).unwrap();
        let player = Self {
            alive: path.join("player-alive"),
            release: path.join("player-release"),
            log: path.join("player-events"),
        };
        std::fs::write(&player.alive, "generated").unwrap();
        std::fs::write(&player.release, "generated").unwrap();
        let executable = bin.join("vlc");
        std::fs::write(
            &executable,
            r#"#!/usr/bin/env python3
import os, pathlib, sys, time, urllib.request
events = pathlib.Path(os.environ['IPTV_GENERATED_PLAYER_EVENTS'])
def record(event):
    with events.open('a') as output:
        output.write(str(os.getpid()) + ' ' + event + '\n')
record('launched')
release = pathlib.Path(os.environ['IPTV_GENERATED_PLAYER_RELEASE'])
while not release.exists():
    time.sleep(0.01)
try:
    url = pathlib.Path(sys.argv[-1]).read_text().splitlines()[-1]
    try:
        with urllib.request.urlopen(url, timeout=10) as response:
            response.read(1)
    except Exception:
        record('rejected')
    else:
        record('admitted')
        while pathlib.Path(os.environ['IPTV_GENERATED_PLAYER_ALIVE']).exists():
            time.sleep(0.01)
finally:
    record('exited')
"#,
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        // SAFETY: this isolated child runs only this scenario. Setup happens
        // before start() creates the app, mock server or their worker threads.
        unsafe {
            std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
            std::env::set_var("IPTV_GENERATED_PLAYER_EVENTS", &player.log);
            std::env::set_var("IPTV_GENERATED_PLAYER_RELEASE", &player.release);
            std::env::set_var("IPTV_GENERATED_PLAYER_ALIVE", &player.alive);
        }
        iptv_app::set_external_player_handler(Some(true));
        player
    }
    fn pids(&self, event: &str) -> Vec<u32> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| {
                let (pid, observed) = line.split_once(' ')?;
                (observed == event).then(|| pid.parse().unwrap())
            })
            .collect()
    }
    fn count(&self, event: &str) -> usize {
        self.pids(event).len()
    }
}

#[cfg(target_os = "linux")]
impl Drop for Player {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.alive);
        let _ = std::fs::write(&self.release, "generated");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn both_archive_entries_handoff_once_and_a_superseded_group_never_reaches_upstream() {
    let Some(home) = isolated!() else { return };
    let player = Player::start(home.path());
    let (mut fixture, server, _sources, at) = start(home);
    let app = &mut fixture.app;
    let internal = iptv_app::player_opens(0);
    for (index, menu) in [false, true].into_iter().enumerate() {
        previous(app);
        if menu {
            menu_replay(app);
        } else {
            step(app.press_key("enter"));
        }
        step(
            app.wait_until("one admitted synthetic external player", WAIT, |_| {
                player.count("admitted") == index + 1
            }),
        );
        assert_eq!(player.count("launched"), index + 1);
        assert_eq!(
            iptv_app::player_opens(0),
            internal,
            "handoff starts no internal player"
        );
        let requests = server.requests();
        assert_eq!(requests.len(), index + 1);
        exact_archive(requests.last().unwrap(), at);
    }

    // Hold the next player's relay request while a different group is selected.
    // This is a real pending host handoff, not an artificial UI-only callback.
    std::fs::remove_file(&player.release).unwrap();
    previous(app);
    menu_replay(app);
    step(
        app.wait_until("third player launched but not admitted", WAIT, |_| {
            player.count("launched") == 3
        }),
    );
    let stale = player.pids("launched")[2];
    personalization::group(app, "Sport");
    std::fs::write(&player.release, "generated").unwrap();
    // A new valid choice settles the retired handoff too; its one request is
    // the only added upstream request, and still identifies channel one's slot.
    personalization::group(app, "News");
    previous(app);
    step(app.press_key("enter"));
    step(app.wait_until("fourth player launched", WAIT, |_| {
        player.count("launched") == 4
    }));
    let accepted = player.pids("launched")[3];
    step(app.wait_until(
        "fourth admitted and third rejected then exited",
        WAIT,
        |_| {
            player.pids("admitted").contains(&accepted)
                && player.pids("rejected").contains(&stale)
                && player.pids("exited").contains(&stale)
        },
    ));
    assert!(
        !player.pids("admitted").contains(&stale),
        "the superseded launch never received media"
    );
    assert_eq!(player.count("admitted"), 3);
    assert_eq!(player.count("launched"), 4);
    assert_eq!(
        server.requests().len(),
        3,
        "stale handoff has no upstream admission"
    );
    exact_archive(server.requests().last().unwrap(), at);
    assert_eq!(iptv_app::player_opens(0), internal);
    drop(player);
}

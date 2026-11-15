//! Synthetic scheduling through the real shell: one workflow for keys and touch.
use crate::headless::Headless;
use crate::personalization::{self, step};
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_settings::{Settings, Value};
use nsh_core::platform::Idiom;
use nsh_core::{Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;
use std::time::Duration;

#[test]
fn recording_capture_failures_have_translations() {
    use iptv_net::recording::Failure;
    for failure in [
        Failure::Network,
        Failure::Unsupported,
        Failure::Storage,
        Failure::Limit,
        Failure::Gap,
    ] {
        assert!(iptv_core::i18n::is_catalogued(failure.message()));
    }
}

fn exercise(home: crate::isolated::Home, idiom: Idiom) {
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    let root = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("settings.json"), settings.to_json_string()).unwrap();
    let path = root.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    let source = store
        .add_source(&Source {
            name: "Generated DVR source".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("https://example.test/generated.m3u"),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 group-title=\"Generated group\",A\nhttps://example.test/generated.ts\n",
    );
    store
        .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
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
    drop(store);
    let mut config = EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    config.environment.idiom = idiom;
    let mut app = Headless::start(config).unwrap();
    step(app.wait_for_within("text:Generated group", Duration::from_secs(20)));
    step(app.press("text:/^☰  /"));
    step(app.press_key("right"));
    // The custom form works without guide data, with the selected channel retained.
    if idiom == Idiom::Tv {
        step(app.press_key("m"));
        for _ in 0..3 {
            step(app.press_key("down"));
        }
        step(app.press_key("enter"));
    } else {
        personalization::menu(&mut app, "Schedule a recording…");
    }
    step(app.wait_for("id:recordings-screen"));
    step(app.wait_for("text:Save recording"));
    // Move the recording to tomorrow through a visible touch/pointer control.
    if idiom == Idiom::Tv {
        for _ in 0..5 {
            step(app.press_key("up"));
        }
        step(app.press_key("right"));
        for _ in 0..5 {
            step(app.press_key("down"));
        }
        step(app.press_key("enter"));
    } else {
        step(app.press("label:/^Increase Start day/"));
        step(app.press("text:Save recording"));
    }
    step(app.wait_for_within("text:/Scheduled ·/", Duration::from_secs(15)));
    let store = crate::isolated::open_database(&path).unwrap();
    let jobs = store.recording_jobs().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].request.source, source.0);
    assert_eq!(jobs[0].status, iptv_core::recording::Status::Scheduled);
    drop(store);
    if idiom == Idiom::Tv {
        step(app.press_key("enter"));
    } else {
        step(app.press("id:recording-job"));
    }
    step(app.wait_for("text:Cancel recording"));
    if idiom == Idiom::Tv {
        // Remote: second action cancels the schedule, then Back returns to the guide.
        step(app.press_key("down"));
        step(app.press_key("enter"));
    } else {
        step(app.press("text:Cancel recording"));
    }
    step(app.wait_for_within("text:/Cancelled ·/", Duration::from_secs(10)));
    if idiom == Idiom::Tv {
        step(app.press_key("enter"));
    } else {
        step(app.press("id:recording-job"));
    }
    if idiom == Idiom::Tv {
        step(app.press_key("enter"));
    } else {
        step(app.press("text:Delete recording"));
    }
    step(app.wait_for("text:/Delete this recording file permanently/"));
    // Closing the whole screen also cancels the pending confirmation. Reopening
    // and choosing Delete must ask again instead of reusing the previous consent.
    if idiom != Idiom::Tv {
        step(app.press("text:Close"));
        step(app.press("text:/^☰  /"));
        step(app.press("label:Recordings"));
        step(app.press("id:recording-job"));
        step(app.press("text:Delete recording"));
        step(app.wait_for("text:/Delete this recording file permanently/"));
    }
    // Cancelling a delete never loses the job.
    step(app.back());
    step(app.wait_for_gone(
        "text:/Delete this recording file permanently/",
        Duration::from_secs(2),
    ));
    assert_eq!(
        crate::isolated::open_database(&path)
            .unwrap()
            .recording_jobs()
            .unwrap()
            .len(),
        1
    );
    if idiom == Idiom::Tv {
        step(app.press_key("enter"));
    } else {
        step(app.press("text:Delete recording"));
    }
    if idiom == Idiom::Tv {
        step(app.press_key("enter"));
    } else {
        step(app.press("text:Delete recording"));
    }
    step(app.wait_for_within("text:/No recordings here/", Duration::from_secs(10)));
    step(app.back());
    step(app.wait_for_gone("id:recordings-screen", Duration::from_secs(2)));
}

#[test]
fn tv_schedules_and_cancels_without_a_keyboard_or_touch_requirement() {
    let Some(home) = isolated!() else {
        return;
    };
    exercise(home, Idiom::Tv);
}
#[test]
fn tablet_schedules_cancels_and_confirms_deletion() {
    let Some(home) = isolated!() else {
        return;
    };
    exercise(home, Idiom::Pad);
}
#[test]
fn desktop_schedules_cancels_and_confirms_deletion() {
    let Some(home) = isolated!() else {
        return;
    };
    exercise(home, Idiom::Desktop);
}

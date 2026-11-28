//! Explicit synthetic native journey driver; compiled only with test-support.
//! Owned by the real root; normal builds have neither a probe nor file writes.
use nsh_core::time::Instant;
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use nsh_core::prelude::*;
use serde_json::{Value, json};

use crate::Ui;

#[derive(Clone, PartialEq)]
pub(crate) struct Probe {
    started: Instant,
    reads: u64,
    opens: u64,
    expected: u64,
    events: Vec<Value>,
    path: PathBuf,
}

/// Only explicit synthetic journeys exclude unrelated periodic view work.
pub(crate) fn enabled() -> bool {
    std::env::var_os("IPTV_PERF_SAMPLE").is_some()
        || std::env::var_os("IPTV_PERF_NATIVE_SAMPLE").is_some()
}

pub(crate) fn bind(ui: Ui) {
    let Some(path) = std::env::var_os("IPTV_PERF_NATIVE_SAMPLE") else {
        return;
    };
    let path = PathBuf::from(path);
    let ready = std::env::var_os("IPTV_PERF_NATIVE_READY").map(PathBuf::from);
    let armed = Rc::new(Cell::new(false));
    let _poll = set_interval(Duration::from_millis(25), move || {
        if armed.get() {
            return;
        }
        if ready.as_ref().is_some_and(|path| !path.exists()) {
            return;
        }
        if !crate::kit::startup::marked("guide frame")
            || ui.app.library.shown.get_untracked().is_none()
            || ui.guide.visible_rows.get_untracked().is_none()
        {
            return;
        }
        armed.set(true);
        let path = path.clone();
        let _settle = set_timeout(Duration::from_millis(500), move || {
            assert!(
                ui.playback.playing.get_untracked().is_none(),
                "synthetic fixture must start without playback"
            );
            ui.guide.focus.update(|focus| {
                focus.pane = crate::guide::Pane::Guide;
                focus.channel = 0;
            });
            let expected = ui.playback.player.main().token.get_untracked() + 1;
            ui.chrome.performance.set(Some(Probe {
                started: Instant::now(),
                reads: crate::store_reads(),
                opens: crate::player_opens(0),
                expected,
                path,
                events: vec![
                    json!({"kind": "start", "at_us": 0, "selection": 1, "token": expected}),
                ],
            }));
            crate::shell::input::synthetic_enter(ui);
        });
    });
}

/// Keep the originating token, including a stale one; the parser rejects it.
pub(crate) fn rendered(ui: Ui, surface: usize, token: u64) {
    if surface != 0 {
        return;
    }
    let mut first = false;
    ui.chrome.performance.update(|probe| {
        if let Some(probe) = probe {
            first = probe.events.len() == 1;
            probe
                .events
                .push(json!({"kind": "first-render", "selection": 1,
                "token": token, "at_us": probe.started.elapsed().as_micros() as u64}));
        }
    });
    if !first {
        return;
    }
    let _observe = set_timeout(Duration::from_millis(500), move || {
        ui.chrome.performance.with_untracked(|probe| {
            let probe = probe.as_ref().expect("active synthetic selection probe");
            let sample = json!({
                "invocation": std::env::var("IPTV_PERF_INVOCATION").ok(),
                "journey": "selection", "build_commit": env!("IPTV_GIT_HASH"),
                "build_mode": if cfg!(debug_assertions) { "debug-test-support" } else { "release-test-support" },
                "events": probe.events, "expected_token": probe.expected,
                "counters": {"store_reads": crate::store_reads() - probe.reads,
                             "player_opens": crate::player_opens(0) - probe.opens},
            });
            // Publish only complete JSON: the native test observes path existence
            // from GTK's main thread while this runs on the engine thread.
            let pending = probe.path.with_extension("pending");
            std::fs::write(&pending, serde_json::to_vec_pretty(&sample).unwrap()).unwrap();
            std::fs::rename(pending, &probe.path).unwrap();
        });
    });
}

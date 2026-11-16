//! Opt-in journey evidence from real isolated scenarios. No media URLs or labels.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nsh_core::MountItem;
use serde_json::json;

/// A monotonic interval, never inferred from missing log lines.
pub(super) struct Sample {
    journey: &'static str,
    selection: u64,
    started: Instant,
    observations: BTreeMap<&'static str, u64>,
}
impl Sample {
    pub(super) fn start(journey: &'static str, selection: u64) -> Self {
        Self {
            journey,
            selection,
            started: Instant::now(),
            observations: BTreeMap::new(),
        }
    }

    pub(super) fn observe(&mut self, name: &'static str, value: u64) {
        self.observations.insert(name, value);
    }

    pub(super) fn elapsed(&self) -> u64 {
        self.started.elapsed().as_micros() as u64
    }

    pub(super) fn finish(self, terminal: &str, token: u64, reads: u64, opens: u64) {
        let at_us = self.elapsed();
        self.finish_at(terminal, token, at_us, reads, opens);
    }

    pub(super) fn finish_at(self, terminal: &str, token: u64, at_us: u64, reads: u64, opens: u64) {
        let Ok(path) = std::env::var("IPTV_PERF_SAMPLE") else {
            return;
        };
        let sample = json!({
            "journey": self.journey,
            "invocation": std::env::var("IPTV_PERF_INVOCATION").ok(),
            "observations": self.observations,
            "build_commit": env!("IPTV_GIT_HASH"),
            "build_mode": if cfg!(debug_assertions) { "debug-test-support" } else { "release-test-support" },
            "events": [
                {"kind": "start", "selection": self.selection, "token": token, "at_us": 0},
                {"kind": terminal, "selection": self.selection,
                 "at_us": at_us, "token": token}
            ],
            "counters": {"store_reads": reads, "player_opens": opens}
        });
        std::fs::write(path, serde_json::to_vec_pretty(&sample).unwrap()).unwrap();
    }
}

#[test]
fn channel_selection_has_one_identified_player_acceptance() {
    let Some(home) = isolated!() else { return };
    let mut fixture = crate::personalization::start(home, iptv_settings::Settings::new(), false);
    fixture.app.idle(Duration::from_millis(500)).unwrap();
    assert!(
        !iptv_app::player_has_source(0),
        "no prior stream before selection"
    );
    let seen = Arc::new(Mutex::new(BTreeMap::new()));
    let into = seen.clone();
    fixture.app.observe(move |_, batch| {
        for item in &batch.items {
            if let MountItem::ComponentProps { props, .. } = item {
                for (key, value) in props {
                    if key.as_ref() == "source"
                        && let Some(token) = value.as_ref().and_then(|v| v["token"].as_u64())
                    {
                        *into.lock().unwrap().entry(token).or_insert(0_u64) += 1;
                    }
                }
            }
        }
    });
    let opens = iptv_app::player_opens(0);
    let reads = iptv_app::store_reads();
    let journey = Sample::start("selection", 1);
    fixture.app.press_key("enter").unwrap();
    fixture
        .app
        .wait_until(
            "identified player acceptance",
            Duration::from_secs(10),
            |_| !seen.lock().unwrap().is_empty(),
        )
        .unwrap();
    let at_us = journey.elapsed();
    fixture.app.idle(Duration::from_millis(500)).unwrap();
    let evidence = seen.lock().unwrap();
    assert_eq!(evidence.len(), 1, "one selected source token");
    let (&token, &count) = evidence.first_key_value().unwrap();
    assert_eq!(count, 1, "one native source submission");
    assert_eq!(
        iptv_app::player_opens(0) - opens,
        1,
        "selection reopened player"
    );
    journey.finish_at(
        "acceptance-proxy",
        token,
        at_us,
        iptv_app::store_reads() - reads,
        iptv_app::player_opens(0) - opens,
    );
}

/// Periodic guide checks do not belong to a single action's work counter.
pub(super) fn prepare(home: &crate::isolated::Home) {
    if std::env::var_os("IPTV_PERF_SAMPLE").is_none() {
        return;
    }
    let directory = home.data_dir();
    std::fs::create_dir_all(&directory).unwrap();
    let mut settings = iptv_settings::Settings::new();
    settings
        .set("epg.update_interval", iptv_settings::Value::Duration(0))
        .unwrap();
    std::fs::write(directory.join("settings.json"), settings.to_json_string()).unwrap();
}

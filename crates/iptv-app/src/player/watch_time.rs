//! Live watch time is sampled from the native player while it advances.
use crate::{Playback, Ui, ViewMode};
use iptv_core::playback::WatchTime;
use iptv_library::SourceId;
use iptv_presentation::playback_health::Phase;
use nsh_core::{platform::Lifecycle, prelude::*};

#[derive(Clone, Debug)]
pub(super) struct Session {
    token: u64,
    source: SourceId,
    key: String,
    time: WatchTime,
}

impl Session {
    fn save(&mut self) {
        let elapsed_ms = self.time.take_ms();
        if elapsed_ms > 0 {
            crate::runtime_owner::runtime().add_channel_watch_time(
                self.source,
                self.key.clone(),
                elapsed_ms,
            );
        }
    }
}

pub(crate) fn bind(ui: Ui) {
    let stream = ui.player.main();
    let eligible = computed(move || {
        (ui.mode.get() == ViewMode::Fullscreen
            && lifecycle() == Lifecycle::Active
            && ui.playback.get() == Playback::Live
            && ui.playing.get().is_some()
            && !ui.paused.get()
            && stream
                .health
                .with(|health| matches!(health.phase(), Phase::Playing)))
        .then(|| stream.token.get())
    });
    effect(move || {
        let token = eligible.get();
        ui.player.watching.with(|watching| {
            if watching
                .as_ref()
                .is_some_and(|session| Some(session.token) == token)
            {
                return;
            }
            if let Some(mut session) = watching.take() {
                session.save();
            }
            if let Some(token) = token
                && let Some(key) = stream.key.get_untracked()
                && let Some((source, key)) =
                    crate::data::combined::owner(&crate::runtime_owner::runtime(), &key)
            {
                *watching = Some(Session {
                    source,
                    key,
                    token,
                    time: WatchTime::default(),
                });
            }
        });
    });

    let guide = computed(move || {
        (ui.mode.get() != ViewMode::Fullscreen)
            .then(|| ui.focus.with(|focus| (focus.group, focus.pane)))
    });
    effect(move || {
        if guide.get().is_some() {
            // A mode change can schedule this effect before the playback effect.
            flush(ui);
            crate::runtime_owner::runtime().refresh_watch_order();
        }
    });
}

pub(crate) fn flush(ui: Ui) {
    if let Some(mut session) = ui.player.watching.replace(None) {
        session.save();
    }
}

pub(crate) fn reset(ui: Ui) {
    ui.player.watching.with(|watching| {
        if let Some(session) = watching {
            session.time = WatchTime::default();
        }
    });
}

pub(crate) fn observe(ui: Ui, surface: usize, position_ms: u64) {
    if surface == 0 && ui.player_surface == 0 {
        ui.player.watching.with(|watching| {
            if let Some(session) = watching
                && session.token == ui.player.main().token.get_untracked()
            {
                session.time.sample(ui.now_ms(), position_ms);
                if session.time.pending_ms() >= 15_000 {
                    session.save();
                }
            }
        });
    }
}

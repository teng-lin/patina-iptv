use crate::guide::home_window;
use crate::{Playback, Ui};
use nsh_core::prelude::*;
use std::cell::Cell;
/// Keeps the archive playing's day held, and drops the other pages once
/// the guide's window is back at now.
pub fn bind(ui: Ui) {
    effect(move || {
        ui.app.data_version.get();
        let archive = match (ui.playback.playing.get(), ui.playback.playback.get()) {
            (Some(channel), Playback::CatchUp { start, .. }) => Some((channel, start)),
            _ => None,
        };
        untracked(|| {
            let schedule = crate::wiring::runtime_owner::schedule();
            iptv_runtime::data::pages::pin_archive(
                &crate::wiring::runtime_owner::runtime(),
                archive.map(|(channel, minute)| {
                    (channel, iptv_core::guide::unix_of(&*schedule, minute))
                }),
            );
        });
    });
    let away = Cell::new(false);
    effect(move || {
        ui.app.data_version.get();
        let home = ui.guide.focus.with(|f| f.window == home_window(f.now));
        if home && away.replace(false) {
            untracked(|| {
                iptv_runtime::data::pages::evict(&crate::wiring::runtime_owner::runtime());
            });
        }
        away.set(!home);
    });
}

pub use iptv_runtime::data::pages::{ready, want};

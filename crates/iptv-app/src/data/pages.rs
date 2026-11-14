use crate::guide::home_window;
use crate::{Playback, Ui};
use nsh_core::prelude::*;
use std::cell::Cell;
/// Keeps the archive playing's day held, and drops the other pages once
/// the guide's window is back at now.
pub fn bind(ui: Ui) {
    effect(move || {
        ui.data_version.get();
        let archive = match (ui.playing.get(), ui.playback.get()) {
            (Some(channel), Playback::CatchUp { start, .. }) => Some((channel, start)),
            _ => None,
        };
        untracked(|| {
            let schedule = crate::schedule();
            crate::runtime::data::pages::pin_archive(
                archive.map(|(channel, minute)| {
                    (channel, iptv_core::guide::unix_of(&*schedule, minute))
                }),
            );
        });
    });
    let away = Cell::new(false);
    effect(move || {
        ui.data_version.get();
        let home = ui.focus.with(|f| f.window == home_window(f.now));
        if home && away.replace(false) {
            untracked(|| {
                crate::runtime::data::pages::evict();
            });
        }
        away.set(!home);
    });
}

pub use crate::runtime::data::pages::{ready, want};

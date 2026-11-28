//! Numeric keys select current displayed numbers, retaining identity until commit.
use crate::Ui;
use iptv_nav::Action;
use nsh_core::prelude::*;
use std::time::Duration;

pub(super) fn bind(ui: Ui) {
    effect(move || {
        // A modal may open and close before the digit timer fires. Cancel on
        // opening so its keys can never resume an abandoned channel number.
        if ui.top_overlay().is_some() || crate::playback::multiview::is_open(ui.playback) {
            ui.playback.channel_number.update(|entry| {
                entry.take();
            });
        }
    });
}

fn timeout() -> u64 {
    crate::wiring::settings::get()
        .duration_ms(
            iptv_settings::keys::DIGIT_TIMEOUT,
            &iptv_settings::Target::Global,
        )
        .max(0) as u64
}
fn resolve(ui: crate::GuideUi, number: u32) -> Option<iptv_search::ChannelId> {
    let schedule = crate::wiring::runtime_owner::schedule();
    let list = ui.list.get_untracked();
    let mut matching = (0..list.len())
        .filter_map(|row| list.channel(row))
        .filter(|&channel| {
            !schedule.channel_hidden(channel) && crate::guide::lists::number(ui, channel) == number
        });
    let channel = matching.next()?;
    if matching.next().is_some() {
        return None;
    }
    crate::wiring::runtime_owner::runtime().search_channel_identity(&*schedule, channel)
}
fn commit(ui: Ui, identity: Option<iptv_search::ChannelId>) {
    let Some(identity) = identity else {
        return;
    };
    let schedule = crate::wiring::runtime_owner::schedule();
    let Some(channel) =
        crate::wiring::runtime_owner::runtime().resolve_search_channel(&identity, &*schedule)
    else {
        return;
    };
    if schedule.channel_hidden(channel)
        || ui
            .guide
            .list
            .with_untracked(|list| list.row_of(channel).is_none())
    {
        return;
    }
    crate::guide::lists::show_channel(ui, channel);
    ui.play(channel, crate::Playback::Live);
    ui.chrome
        .mode
        .apply(crate::kit::mode::Action::EnterFullscreen);
    ui.show_playing();
}
fn arm(ui: Ui) {
    let digits = ui
        .playback
        .channel_number
        .with(|entry| entry.digits().to_owned());
    if !digits.is_empty() {
        crate::kit::toast::notify(&digits);
    }
    let _task = set_timeout(Duration::from_millis(timeout()), move || {
        if ui.top_overlay().is_some() || crate::playback::multiview::is_open(ui.playback) {
            ui.playback.channel_number.update(|entry| {
                entry.take();
            });
            return;
        }
        let mut target = None;
        ui.playback
            .channel_number
            .update(|entry| target = entry.expire(ui.now_ms()));
        commit(ui, target);
    });
}
pub(super) fn apply(ui: Ui, action: Action) -> bool {
    if ui.playback.player_surface != 0 {
        return false;
    }
    match action {
        Action::Digit(digit) => {
            ui.playback.channel_number.update(|entry| {
                entry.push(digit, ui.now_ms(), timeout(), |number| {
                    resolve(ui.guide, number)
                })
            });
            arm(ui);
            true
        }
        Action::Select if ui.playback.channel_number.with(|entry| entry.active()) => {
            let mut target = None;
            ui.playback
                .channel_number
                .update(|entry| target = entry.take());
            commit(ui, target);
            true
        }
        _ => {
            ui.playback.channel_number.update(|entry| {
                entry.take();
            });
            false
        }
    }
}
pub(super) fn backspace(ui: Ui) -> bool {
    if !ui.playback.channel_number.with(|entry| entry.active()) {
        return false;
    }
    ui.playback.channel_number.update(|entry| {
        entry.backspace(ui.now_ms(), timeout(), |number| resolve(ui.guide, number))
    });
    arm(ui);
    true
}

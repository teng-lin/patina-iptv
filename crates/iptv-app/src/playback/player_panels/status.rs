//! Waiting and error states over the video (`player.rs`'s health): a
//! small pill while loading, buffering or reconnecting; a card with Try
//! again once retries stopped (OK retries too). Compact in the guide's
//! preview.

use crate::chrome::i18n::{tr, trf};

use iptv_presentation::playback_health::Phase;
use nsh_core::prelude::*;

use super::words::{failure_message, waiting_text};
use super::{PANEL_ALPHA, WHITE};
use crate::chrome::fonts::{OSD_BADGE, OSD_TEXT, OSD_TITLE};
use crate::theme::{FOCUS_ALPHA, RADIUS, RADIUS_LARGE, TEXT_SECONDARY_ALPHA};
use crate::{Playback, Ui, ViewMode, channel_name, tint};

/// What the status layer shows over the video.
#[derive(Clone, Debug, PartialEq)]
enum Status {
    Hidden,
    Waiting(String),
    Failed {
        title: String,
        message: String,
        detail: String,
    },
    /// A replayed program reached its end.
    Over,
}

fn status_of(ui: Ui) -> Status {
    let channel = ui.playback.playing.get();
    let demand = crate::playback::on_demand::active(ui.playback);
    if channel.is_none() && !demand {
        return Status::Hidden;
    }
    if ui.chrome.mode.get() == ViewMode::Overlay {
        return Status::Hidden; // the guide covers the video
    }
    if crate::playback::multiview::is_open(ui.playback) {
        return Status::Hidden; // each tile shows its own
    }
    let detail_text = |detail: &str| match detail {
        "no stream" => tr("no stream").to_owned(),
        "no channel" => tr("no channel").to_owned(),
        "no account" => tr("no account").to_owned(),
        _ => detail.to_owned(),
    };
    let catch_up = matches!(ui.playback.playback.get(), Playback::CatchUp { .. });
    let now = ui.now_ms();
    ui.playback
        .player
        .main()
        .health
        .with(|h| match (h.phase(), h.failure()) {
            (Phase::Idle, _) if catch_up || demand => Status::Over,
            // Retries may go on behind the card: it goes once the stream plays.
            (_, Some(f)) => Status::Failed {
                title: trf(
                    "Can't play {name}",
                    &[(
                        "name",
                        crate::playback::on_demand::title(ui.playback)
                            .unwrap_or_else(|| channel_name(channel.unwrap_or(0)))
                            .as_str()
                            .into(),
                    )],
                ),
                message: failure_message(&f.kind, catch_up),
                detail: match (f.attempts, f.retrying) {
                    (_, true) => trf(
                        "{detail} · still trying",
                        &[("detail", detail_text(&f.detail).as_str().into())],
                    ),
                    (0, false) => detail_text(&f.detail),
                    (n, false) => trf(
                        "{count, plural, one {{detail} · tried # time} \
                         other {{detail} · tried # times}}",
                        &[
                            ("detail", detail_text(&f.detail).as_str().into()),
                            ("count", n.into()),
                        ],
                    ),
                },
            },
            // A quick reconnect keeps the last frame without a pill.
            _ => h
                .indicator(now)
                .map_or(Status::Hidden, |w| Status::Waiting(waiting_text(&w))),
        })
}

/// The status over the video, in the player's rectangle.
pub fn status(ui: Ui) -> impl IntoElement {
    let status = computed(move || status_of(ui));
    let compact = move || ui.chrome.mode.get() == ViewMode::Guide;
    let panel = move || tint(ui.app.theme.with(|t| t.background.panel), PANEL_ALPHA);
    let size = move |full: f32| px(if compact() { full * 0.6 } else { full });
    let waiting = move || matches!(status.get(), Status::Waiting(_));
    let card = move || matches!(status.get(), Status::Failed { .. } | Status::Over);
    let secondary = move || tint(WHITE, TEXT_SECONDARY_ALPHA);
    let retry_active = move || {
        !ui.any_overlay_open()
            && !crate::playback::play_bar::captures(ui)
            && !ui.playback.panels.with(|p| p.captures_input())
    };
    let retry_keys = move || ui.keys_shown() && retry_active();
    view()
        .accessibility_elements_hidden(move || ui.any_overlay_open())
        .absolute()
        .left(move || px(crate::playback::video::rect(ui).0))
        .top(move || px(crate::playback::video::rect(ui).1))
        .w(move || px(crate::playback::video::rect(ui).2))
        .h(move || px(crate::playback::video::rect(ui).3))
        .items_center()
        .justify_center()
        .pointer_events(PointerEvents::BoxNone)
        .child(when(waiting, move || {
            text(move || match status.get() {
                Status::Waiting(t) => t,
                _ => String::new(),
            })
            .px(size(18.0))
            .py(size(8.0))
            .rounded(px(RADIUS_LARGE))
            .bg(panel)
            .text_size(move || size(OSD_TEXT))
            .text_color(rgb(WHITE))
            // Taps go through to the video (the play bar).
            .pointer_events(PointerEvents::None)
        }))
        .child(when(card, move || {
            let failed = move || matches!(status.get(), Status::Failed { .. });
            let full =
                move || (failed() || crate::playback::on_demand::active(ui.playback)) && !compact();
            view()
                .items_center()
                .gap(size(10.0))
                .px(size(28.0))
                .py(size(20.0))
                .rounded(px(RADIUS_LARGE))
                .bg(panel)
                .pointer_events(PointerEvents::Auto)
                .child(
                    text(move || match status.get() {
                        Status::Failed { title, .. } => title,
                        _ if crate::playback::on_demand::active(ui.playback) => {
                            tr("Playback finished").into()
                        }
                        _ => tr("The program has ended").to_owned(),
                    })
                    .text_size(move || size(OSD_TITLE))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(WHITE)),
                )
                .child(
                    text(move || match status.get() {
                        Status::Failed { message, .. } => message,
                        _ if crate::playback::on_demand::active(ui.playback) => String::new(),
                        _ => tr("Next (⏭) returns to live.").to_owned(),
                    })
                    .text_size(move || size(OSD_TEXT))
                    .text_color(secondary()),
                )
                .child(when(full, move || {
                    text(move || match status.get() {
                        Status::Failed { detail, .. } => detail,
                        _ => String::new(),
                    })
                    .text_size(px(OSD_BADGE))
                    .text_color(secondary())
                }))
                .child(when(full, move || {
                    text(move || {
                        if crate::playback::on_demand::active(ui.playback) && !failed() {
                            tr("Play again").to_owned()
                        } else if retry_keys() {
                            tr("OK  Try again").to_owned()
                        } else {
                            tr("Try again").to_owned()
                        }
                    })
                    .test_id("player-retry")
                    .role(AccessibilityRole::Button)
                    .accessibility_state(move || AccessibilityState {
                        selected: retry_keys(),
                        ..Default::default()
                    })
                    .mt(px(6.0))
                    .px(px(20.0))
                    .py(px(8.0))
                    .rounded(px(RADIUS))
                    .text_size(px(OSD_TEXT))
                    .text_color(rgb(WHITE))
                    .bg(move || {
                        tint(
                            ui.app.theme.with(|t| t.accent.main),
                            if retry_active() {
                                FOCUS_ALPHA
                            } else {
                                crate::theme::RESTING_ALPHA
                            },
                        )
                    })
                    .border(px(2.0))
                    .border_color(move || {
                        tint(
                            ui.app.theme.with(|t| t.accent.light),
                            if retry_active() { 255 } else { 0 },
                        )
                    })
                    .on_press(move || {
                        ui.touched();
                        if crate::playback::on_demand::active(ui.playback) && !failed() {
                            crate::playback::player::toggle_pause(ui);
                        } else {
                            crate::playback::player::retry(ui);
                        }
                    })
                }))
        }))
}

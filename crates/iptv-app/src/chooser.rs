//! Translucent choosers over full-screen video: picture size (with "apply
//! to all channels"), the sleep timer, the video, audio and subtitle
//! tracks, the audio delay, and the recently watched channels (the History
//! quick action: the store's watch history, as the Recently watched list).
//! Option sets follow the choosers observed in the reference UX;
//! wording is ours. Entries are taken when a chooser
//! opens; Up/Down move, OK (or a tap) picks and closes, Back closes.

use crate::i18n::{tr, trf};

use std::time::Duration;

use iptv_core::player_options::{Chooser, PictureSize, SLEEP_MINUTES};
use iptv_core::sound::{audio_delays, delay_label};
use iptv_core::tracks::Kind as Track;
use nsh_core::Checked;
use nsh_core::prelude::*;

use crate::list_rows::{self, Mark, ROW};
use crate::scale::Scale;
use crate::theme::RADIUS_LARGE;
use crate::ui_kit;
use crate::{Playback, Ui, ViewMode, tint};

/// Rows shown at once; longer lists scroll (seven fit a TV's 540 dp).
const MAX_ROWS: usize = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Picture,
    Sleep,
    Track(Track),
    AudioDelay,
    /// The recently watched channels.
    History,
}

/// The open chooser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Open {
    pub kind: Kind,
    pub labels: Vec<String>,
    /// What each entry sets, for tracks (a track id, `auto`, `off`) and
    /// history (a channel key).
    values: Vec<String>,
    pub list: Chooser,
}

fn picture_label(size: PictureSize) -> &'static str {
    match size {
        PictureSize::Normal => "Normal",
        PictureSize::Full => "Stretch to fill",
        PictureSize::Zoom => "Zoom",
        PictureSize::CinemaZoom => "Zoom wide films",
        PictureSize::Reduced => "Smaller",
        PictureSize::Ratio4x3 => "4:3",
        PictureSize::Ratio16x9 => "16:9",
        PictureSize::Ratio235 => "2.35:1",
    }
}

/// The current picture size's name (the control row shows it).
pub fn picture_name(ui: Ui) -> &'static str {
    let ch = ui.playing.get().unwrap_or(0);
    picture_label(ui.picture.with(|p| p.for_channel(ch)))
}

fn title(kind: Kind) -> &'static str {
    match kind {
        Kind::Picture => tr("Picture size"),
        Kind::Sleep => tr("Sleep timer"),
        Kind::Track(Track::Video) => tr("Video"),
        Kind::Track(Track::Audio) => tr("Audio"),
        Kind::Track(Track::Text) => tr("Subtitles"),
        Kind::AudioDelay => tr("Audio delay"),
        Kind::History => tr("Recently watched"),
    }
}

/// The entries, what each sets, and the one in effect.
fn entries(ui: Ui, kind: Kind) -> (Vec<String>, Vec<String>, usize) {
    match kind {
        Kind::Picture => {
            let ch = ui.playing.get_untracked().unwrap_or(0);
            let size = ui.picture.with_untracked(|p| p.for_channel(ch));
            let labels = PictureSize::ALL
                .iter()
                .map(|p| picture_label(*p).to_owned())
                .chain(["Use for all channels".to_owned()])
                .collect();
            let at = PictureSize::ALL.iter().position(|p| *p == size);
            (labels, Vec::new(), at.unwrap_or(0))
        }
        Kind::Sleep => {
            let labels = [tr("Off").to_owned()]
                .into_iter()
                .chain(SLEEP_MINUTES.iter().map(|m| {
                    trf(
                        "{count, plural, one {# minute} other {# minutes}}",
                        &[("count", (*m).into())],
                    )
                }))
                .collect();
            let at = ui
                .sleep
                .with_untracked(|t| t.minutes())
                .and_then(|m| SLEEP_MINUTES.iter().position(|x| *x == m))
                .map_or(0, |i| i + 1);
            (labels, Vec::new(), at)
        }
        Kind::Track(k) => ui.player.main().tracks.with_untracked(|t| {
            let labels = t.entries(k);
            let values = (0..labels.len())
                .map(|i| t.pick(k, i).unwrap_or_default())
                .collect();
            (labels, values, t.selected_entry(k))
        }),
        Kind::AudioDelay => {
            let now = ui.media.delay_ms.get_untracked();
            let delays = audio_delays();
            let labels = delays
                .iter()
                .map(|d| delay_label(crate::i18n::language(), *d))
                .collect();
            let at = delays.iter().position(|d| *d >= now);
            (labels, Vec::new(), at.unwrap_or(delays.len() / 2))
        }
        Kind::History => history(ui),
    }
}

/// The recently watched channels (the store's watch history, as in the
/// Recently watched list), newest first, numbered as on the player card:
/// "12  Name ▶ · now". The focus starts on the one playing.
fn history(ui: Ui) -> (Vec<String>, Vec<String>, usize) {
    let s = crate::schedule();
    let now = ui.focus.with_untracked(|f| f.now);
    let h = crate::favorites::history(ui.playing.get_untracked());
    let labels = h
        .channels
        .iter()
        .enumerate()
        .map(|(row, &ch)| {
            let p = s.program_at(ch, now);
            let marker = if h.playing == Some(row) { "  ▶" } else { "" };
            format!(
                "{}  {}{marker} · {}",
                crate::lists::number(ui, ch),
                s.channel_name(ch),
                s.title(ch, p)
            )
        })
        .collect();
    let keys = h
        .channels
        .iter()
        .map(|&ch| s.channel_key(ch).unwrap_or_default())
        .collect();
    (labels, keys, h.playing.unwrap_or(0))
}

pub fn open(ui: Ui, kind: Kind) {
    let (labels, values, selected) = entries(ui, kind);
    if labels.is_empty() {
        return;
    }
    ui.chooser.set(Some(Open {
        kind,
        list: Chooser::new(labels.len(), selected),
        labels,
        values,
    }));
}

pub fn close(ui: Ui) {
    ui.chooser.set(None);
}

/// Opens `kind`, or closes it when it is the one open (a pointer's second
/// press on the button that opened it).
pub fn toggle(ui: Ui, kind: Kind) {
    if ui.chooser.with_untracked(|c| c.as_ref().map(|c| c.kind)) == Some(kind) {
        close(ui);
    } else {
        open(ui, kind);
    }
}

pub fn move_focus(ui: Ui, step: isize) {
    ui.chooser
        .update(|c| c.iter_mut().for_each(|c| c.list.move_focus(step)));
}

/// OK (or a tap) on entry `index`.
pub fn pick(ui: Ui, index: usize) {
    let Some(open) = ui.chooser.get_untracked() else {
        return;
    };
    let value = open.values.get(index).cloned().unwrap_or_default();
    close(ui);
    match open.kind {
        Kind::Picture => {
            let ch = ui.playing.get_untracked().unwrap_or(0);
            match PictureSize::ALL.get(index) {
                Some(size) => ui.picture.update(|p| p.set_channel(ch, *size)),
                None => ui.picture.update(|p| p.apply_to_all(ch)),
            }
        }
        Kind::Sleep => match index.checked_sub(1).and_then(|i| SLEEP_MINUTES.get(i)) {
            None => ui.sleep.update(|t| t.cancel()),
            Some(minutes) => start_sleep(ui, *minutes),
        },
        Kind::Track(k) if !value.is_empty() => crate::media::choose(ui, k, value),
        Kind::Track(_) => {}
        Kind::AudioDelay => {
            if let Some(ms) = audio_delays().get(index) {
                ui.media.delay_ms.set(*ms);
            }
        }
        Kind::History => play_recent(ui, &value),
    }
}

/// History: plays a recent channel live, full screen, with its card.
fn play_recent(ui: Ui, key: &str) {
    let Some(channel) = crate::favorites::channel_of(key) else {
        return;
    };
    crate::lists::show_channel(ui, channel);
    let live = ui.playback.get_untracked() == Playback::Live;
    if ui.playing.get_untracked() != Some(channel) || !live {
        ui.play(channel, Playback::Live);
    }
    ui.mode.set(ViewMode::Fullscreen);
    ui.update_panels(|p, now| {
        p.hide();
        p.show_banner(now);
    });
}

/// Starts the sleep timer; when it fires, playback stops.
fn start_sleep(ui: Ui, minutes: u32) {
    let now = ui.now_ms();
    ui.sleep.update(|t| t.set(minutes, now));
    let _task = set_timeout(Duration::from_secs(u64::from(minutes) * 60), move || {
        let mut fired = false;
        let now = ui.now_ms();
        ui.sleep.update(|t| fired = t.fire_if_due(now));
        if fired {
            log::info!("iptv sleep timer: stopping playback");
            crate::multiview::close(ui);
            ui.playing.set(None);
            ui.mode.set(crate::settings::browse_mode());
        }
    });
}

pub fn layer(ui: Ui) -> impl IntoElement {
    let open = move || ui.chooser.with(Option::is_some);
    let count = move || {
        ui.chooser
            .with(|c| c.as_ref().map_or(0, |c| c.labels.len()))
    };
    let size = Scale::new();
    let list = element_ref();
    let focus = computed(move || ui.chooser.with(|c| c.as_ref().map(|c| c.list.focus)));
    effect(move || {
        if let Some(i) = focus.get() {
            list.scroll_to_index(i as u32, percentage(0.5), px(0.0), false);
        }
    });
    let row = move |at: Position| {
        // Cells are recycled while a long list scrolls: read `at` where used.
        let entry = move || {
            ui.chooser.with(|c| {
                c.as_ref().and_then(|c| {
                    let index = at.index();
                    let label = c.labels.get(index)?.clone();
                    let checked = c.kind != Kind::History && c.list.selected == index;
                    Some((label, checked, c.list.focus == index))
                })
            })
        };
        let selected = move || entry().is_some_and(|(_, checked, _)| checked);
        let mark = move || Mark::of(ui.keys_shown() && entry().is_some_and(|(_, _, f)| f), false);
        list_rows::row(size, ROW, mark)
            .role(AccessibilityRole::Option)
            .accessibility_state(move || AccessibilityState {
                selected: selected(),
                checked: Some(if selected() {
                    Checked::True
                } else {
                    Checked::False
                }),
                ..Default::default()
            })
            .on_press(move || {
                ui.touched();
                pick(ui, at.index());
            })
            .child(
                list_rows::name(size, move || {
                    entry().map_or(String::new(), |(label, selected, _)| {
                        format!("{}{label}", if selected { "✓  " } else { "    " })
                    })
                })
                .text_color(move || mark().text()),
            )
    };
    let kind = move || ui.chooser.with(|c| c.as_ref().map(|c| c.kind));
    // Mounted while closed: nothing in it says `PointerEvents::Auto`
    // (`channels_view::layer`).
    view()
        .absolute()
        .inset_0()
        // At the right, clear of the panels at the bottom.
        .items_end()
        .justify_center()
        .pt_safe_area()
        .p(px(40.0))
        // Clear of the play bar while it shows.
        .pb(move || {
            px(if crate::play_bar::shows(ui) {
                crate::play_bar::height() + 16.0
            } else {
                40.0
            })
        })
        .test_id("chooser-layer")
        .accessibility_elements_hidden(move || {
            crate::overlays::Overlay::Chooser.accessibility_hidden(ui)
        })
        .opacity(move || if open() { 1.0_f32 } else { 0.0 })
        .pointer_events(move || {
            if open() {
                PointerEvents::BoxNone
            } else {
                PointerEvents::None
            }
        })
        // A tap beside the chooser closes it (the play bar, over this
        // layer, keeps its own presses).
        .child(
            view()
                .absolute()
                .inset_0()
                .label(move || tr("Close the chooser"))
                .on_press(move || {
                    ui.touched();
                    close(ui);
                }),
        )
        .child(
            view()
                .w(move || {
                    size.px(if kind() == Some(Kind::History) {
                        520.0
                    } else {
                        380.0
                    })
                })
                .p(px(12.0))
                .gap(px(2.0))
                .rounded(px(RADIUS_LARGE))
                // Translucent over moving video, like the reference's choosers.
                .bg(move || tint(ui.theme.with(|t| t.background.panel), 0xD0))
                .child(
                    ui_kit::heading(size, move || kind().map_or("", title))
                        .px(move || size.px(list_rows::INSET))
                        .py(move || size.px(8.0)),
                )
                .child(
                    virtual_list(count, row)
                        .estimated_item_size(list_rows::pitch(size, ROW))
                        .element_ref(&list)
                        .h(move || px(count().clamp(1, MAX_ROWS) as f32 * size.whole(ROW))),
                ),
        )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn every_chooser_and_picture_size_has_its_own_name() {
        let kinds = [
            Kind::Picture,
            Kind::Sleep,
            Kind::Track(Track::Video),
            Kind::Track(Track::Audio),
            Kind::Track(Track::Text),
            Kind::AudioDelay,
            Kind::History,
        ];
        let titles: BTreeSet<_> = kinds.iter().map(|k| title(*k)).collect();
        assert_eq!(titles.len(), kinds.len());
        assert_eq!(title(Kind::Track(Track::Text)), "Subtitles");
        let sizes: BTreeSet<_> = PictureSize::ALL.iter().map(|p| picture_label(*p)).collect();
        assert_eq!(sizes.len(), PictureSize::ALL.len());
    }
}

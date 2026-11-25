//! Translucent choosers over full-screen video: picture size (with "apply
//! to all channels"), the sleep timer, the video, audio and subtitle
//! tracks, the audio delay, the recently watched channels (the History
//! quick action: the store's watch history, as the Recently watched list),
//! and the question before leaving where the host asks it
//! (`crate::confirm_leaving`).
//! Option sets follow the choosers observed in the reference UX;
//! wording is ours. More follows the available controls; Up/Down move,
//! OK (or a tap) picks and closes, Back closes.

use crate::i18n::{tr, trf};

use std::time::Duration;

use iptv_core::tracks::Kind as Track;
use iptv_presentation::player_options::{Chooser, PictureSize, SLEEP_MINUTES};
use iptv_presentation::sound::{audio_delays, delay_label};
use iptv_presentation::tracks::entries_in;
use nsh_core::Checked;
use nsh_core::prelude::*;

use crate::list_rows::{self, Mark, ROW};
use crate::scale::Scale;
use crate::theme::RADIUS_LARGE;
use crate::ui_kit;
use crate::{Playback, Ui, tint};

/// Rows shown at once; longer lists scroll (seven fit a TV's 540 dp).
const MAX_ROWS: usize = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Picture,
    More,
    Volume,
    Sleep,
    Track(Track),
    AudioDelay,
    /// The recently watched channels.
    History,
    /// No or Yes before the app leaves (`crate::confirm_leaving`).
    Leave,
}

/// The open chooser.
#[derive(Clone, Debug, PartialEq)]
pub struct Open {
    pub kind: Kind,
    pub labels: Vec<String>,
    /// What each entry sets, for tracks (a track id, `auto`, `off`) and
    /// history (a channel key).
    values: Vec<String>,
    /// Snapshot behind both the translated labels and their opaque pick values.
    tracks: Option<iptv_core::tracks::Tracks>,
    // Preserve action identity for a press and keep the originating desktop window.
    more: Option<More>,
    pub list: Chooser,
}

#[derive(Clone, Debug, PartialEq)]
struct More {
    controls: Vec<iptv_presentation::play_bar::Control>,
    window: Option<nsh_core::element::desktop::WindowRef>,
}

pub(super) fn open_more(
    ui: Ui,
    controls: Vec<iptv_presentation::play_bar::Control>,
    window: Option<nsh_core::element::desktop::WindowRef>,
) {
    if controls.is_empty() {
        return;
    }
    ui.chooser.set(Some(Open {
        kind: Kind::More,
        list: Chooser::new(controls.len(), 0),
        labels: Vec::new(),
        values: Vec::new(),
        tracks: None,
        more: Some(More { controls, window }),
    }));
}

fn picture_label(size: PictureSize) -> &'static str {
    match size {
        PictureSize::Normal => tr("Normal"),
        PictureSize::Full => tr("Stretch to fill"),
        PictureSize::Zoom => tr("Zoom"),
        PictureSize::CinemaZoom => tr("Zoom wide films"),
        PictureSize::Reduced => tr("Smaller"),
        PictureSize::Ratio4x3 => "4:3",
        PictureSize::Ratio16x9 => "16:9",
        PictureSize::Ratio235 => "2.35:1",
    }
}

/// Stable source-scoped identity; on-demand content uses the global size.
pub(crate) fn picture_identity(ui: Ui, surface: usize) -> Option<String> {
    if surface == 0 && crate::on_demand::active(ui) {
        return None;
    }
    let key = ui.player.stream(surface).key.get()?;
    let schedule = crate::runtime_owner::schedule();
    let channel = schedule.find_channel(&key)?;
    let identity = crate::runtime_owner::runtime().search_channel_identity(&*schedule, channel)?;
    Some(serde_json::json!([identity.source, identity.incarnation(), identity.content]).to_string())
}

/// The current picture size's name (the control row shows it).
pub fn picture_name(ui: Ui) -> &'static str {
    let ch = picture_identity(ui, ui.player_surface);
    picture_label(ui.app.picture.with(|p| p.for_channel(ch.as_deref())))
}

fn title(kind: Kind) -> &'static str {
    match kind {
        Kind::Picture => tr("Picture size"),
        Kind::More => tr("More options"),
        Kind::Volume => tr("Volume"),
        Kind::Sleep => tr("Sleep timer"),
        Kind::Track(Track::Video) => tr("Video"),
        Kind::Track(Track::Audio) => tr("Audio"),
        Kind::Track(Track::Text) => tr("Subtitles"),
        Kind::AudioDelay => tr("Audio delay"),
        Kind::History => tr("Recently watched"),
        Kind::Leave => tr("Close Patina?"),
    }
}

fn volume_index(ui: Ui) -> usize {
    (usize::from(crate::media::audible_volume(ui)) + 5) / 10
}

/// The entries, what each sets, and the one in effect.
fn entries(ui: Ui, kind: Kind) -> (Vec<String>, Vec<String>, usize) {
    match kind {
        Kind::Picture => {
            let ch = picture_identity(ui, ui.player_surface);
            let size = ui
                .app
                .picture
                .with_untracked(|p| p.for_channel(ch.as_deref()));
            let labels = PictureSize::ALL
                .iter()
                .map(|p| picture_label(*p).to_owned())
                .chain([tr("Use for all channels").to_owned()])
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
            let labels = entries_in(t, k, crate::i18n::language());
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
        // No first, and focused: OK alone never leaves.
        Kind::Leave => (
            vec![tr("No").to_owned(), tr("Yes").to_owned()],
            Vec::new(),
            0,
        ),
        Kind::More => (Vec::new(), Vec::new(), 0),
        Kind::Volume => (
            (0..=10).map(|n| format!("{}%", n * 10)).collect(),
            Vec::new(),
            volume_index(ui),
        ),
    }
}

/// The recently watched channels (the store's watch history, as in the
/// Recently watched list), newest first, numbered as on the player card:
/// "12  Name ▶ · now". The focus starts on the one playing.
fn history(ui: Ui) -> (Vec<String>, Vec<String>, usize) {
    let s = crate::runtime_owner::schedule();
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
                crate::listing_labels::listing(s.listing(ch, p))
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

impl Open {
    fn for_tracks(kind: Track, tracks: iptv_core::tracks::Tracks) -> Self {
        let labels = entries_in(&tracks, kind, crate::i18n::language());
        let values = (0..labels.len())
            .map(|i| tracks.pick(kind, i).unwrap_or_default())
            .collect();
        Self {
            kind: Kind::Track(kind),
            list: Chooser::new(labels.len(), tracks.selected_entry(kind)),
            labels,
            values,
            tracks: Some(tracks),
            more: None,
        }
    }

    fn track_labels_in(&self, language: &iptv_presentation::i18n::Language) -> Vec<String> {
        match (self.kind, self.tracks.as_ref()) {
            (Kind::Track(kind), Some(tracks)) => entries_in(tracks, kind, language),
            _ => Vec::new(),
        }
    }
}

pub fn open(ui: Ui, kind: Kind) {
    if let Kind::Track(kind) = kind {
        let tracks = ui.player.main().tracks.get_untracked();
        let open = Open::for_tracks(kind, tracks);
        if !open.labels.is_empty() {
            ui.chooser.set(Some(open));
        }
        return;
    }
    let (labels, values, selected) = entries(ui, kind);
    if labels.is_empty() {
        return;
    }
    ui.chooser.set(Some(Open {
        kind,
        list: Chooser::new(labels.len(), selected),
        labels,
        values,
        tracks: None,
        more: None,
    }));
}

pub fn close(ui: Ui) {
    ui.chooser.set(None);
}

/// Back at the top level, where the host asks before leaving: No or Yes,
/// with No focused. Always handled.
pub(crate) fn ask_to_leave(ui: Ui) -> bool {
    open(ui, Kind::Leave);
    true
}

pub fn move_focus(ui: Ui, step: isize) {
    ui.chooser
        .update(|c| c.iter_mut().for_each(|c| c.list.move_focus(step)));
}

/// Left selects No and Right selects Yes without wrapping on repeats.
pub(crate) fn focus_leave(ui: Ui, yes: bool) {
    ui.chooser.update(|open| {
        if let Some(open) = open.as_mut().filter(|open| open.kind == Kind::Leave) {
            open.list.focus = usize::from(yes);
        }
    });
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
            let ch = picture_identity(ui, ui.player_surface);
            match PictureSize::ALL.get(index) {
                Some(size) => ui
                    .app
                    .picture
                    .update(|p| p.set_channel(ch.as_deref(), *size)),
                None => ui.app.picture.update(|p| p.apply_to_all(ch.as_deref())),
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
        Kind::Volume => {
            if index <= 10 {
                crate::media::set_volume(ui, index as i32 * 10);
            }
        }
        Kind::More => {
            if let Some(more) = open.more
                && let Some(&control) = more.controls.get(index)
            {
                crate::play_bar::activate(ui, control, more.window);
            }
        }
        Kind::History => play_recent(ui, &value),
        Kind::Leave => {
            if index == 1 {
                crate::leave();
            }
        }
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
    ui.mode.apply(crate::mode::Action::EnterFullscreen);
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
            crate::player_keys::stop_playback(ui);
        }
    });
}

/// Whether a chooser is open in the layer of the question before leaving
/// (`leave`, over every screen) or in the one of the others (over the
/// video, under the play bar).
pub(crate) fn is_open(ui: Ui, leave: bool) -> bool {
    ui.chooser
        .with(|c| c.as_ref().is_some_and(|c| (c.kind == Kind::Leave) == leave))
}

/// The choosers' layer: `leave` for the question before leaving, over
/// every screen; else the others, over the video. Each shows only its own.
pub fn layer(ui: Ui, leave: bool) -> impl IntoElement {
    let open = move || is_open(ui, leave);
    let overlay = if leave {
        crate::overlays::Overlay::Leave
    } else {
        crate::overlays::Overlay::Chooser
    };
    let width = signal(None);
    if !leave {
        let more_open = computed(move || {
            ui.chooser
                .with(|chooser| chooser.as_ref().is_some_and(|c| c.kind == Kind::More))
        });
        effect(move || {
            if !more_open.get() {
                return;
            }
            let Some(width) = width.get() else {
                return;
            };
            let controls = crate::play_bar::overflow(ui, width);
            if controls.is_empty() {
                close(ui);
            } else if ui.chooser.with_untracked(|chooser| {
                chooser
                    .as_ref()
                    .and_then(|c| c.more.as_ref())
                    .is_some_and(|more| more.controls != controls)
            }) {
                ui.chooser.update(|chooser| {
                    if let Some(open) = chooser
                        && let Some(more) = &mut open.more
                    {
                        open.list.replace_actions(&more.controls, &controls);
                        more.controls = controls;
                    }
                });
            }
        });
    }
    let count = move || {
        if open() {
            ui.chooser.with(|c| c.as_ref().map_or(0, |c| c.list.len))
        } else {
            0
        }
    };
    let size = Scale::new();
    let list = element_ref();
    let focus = computed(move || {
        open()
            .then(|| ui.chooser.with(|c| c.as_ref().map(|c| c.list.focus)))
            .flatten()
    });
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
                    let label = match c.kind {
                        Kind::More => {
                            crate::play_bar::label(ui, *c.more.as_ref()?.controls.get(index)?)
                                .to_owned()
                        }
                        Kind::History | Kind::Volume => c.labels.get(index)?.clone(),
                        Kind::Track(_) => c
                            .track_labels_in(crate::i18n::language())
                            .get(index)?
                            .clone(),
                        _ => entries(ui, c.kind).0.get(index)?.clone(),
                    };
                    let checked = match c.kind {
                        Kind::History | Kind::More | Kind::Leave => false,
                        Kind::Volume => volume_index(ui) == index,
                        _ => c.list.selected == index,
                    };
                    Some((label, checked, c.list.focus == index))
                })
            })
        };
        let selected = move || entry().is_some_and(|(_, checked, _)| checked);
        let mark = move || Mark::of(ui.keys_shown() && entry().is_some_and(|(_, _, f)| f), false);
        let action = move || {
            ui.chooser.with(|c| {
                c.as_ref()
                    .is_some_and(|c| matches!(c.kind, Kind::More | Kind::Leave))
            })
        };
        list_rows::row(size, ROW, mark)
            .role(move || {
                if action() {
                    AccessibilityRole::Button
                } else {
                    AccessibilityRole::Option
                }
            })
            .accessibility_state(move || AccessibilityState {
                selected: selected(),
                checked: (!action()).then(|| {
                    if selected() {
                        Checked::True
                    } else {
                        Checked::False
                    }
                }),
                ..Default::default()
            })
            .on_press(move || {
                ui.touched();
                // The row unmounts on selection. Playback actions and timers
                // belong to the window where the chooser opened.
                ui.owner.run(|| pick(ui, at.index()));
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
    let kind = move || {
        open()
            .then(|| ui.chooser.with(|c| c.as_ref().map(|c| c.kind)))
            .flatten()
    };
    // Mounted while closed: nothing in it says `PointerEvents::Auto`
    // (`channels_view::layer`).
    view()
        .absolute()
        .inset_0()
        // This layer still measures the local window when a short chooser
        // hides the bar, including in an auxiliary player window.
        .on_layout(move |bounds: &Bounds<Points>| {
            let measured = (bounds.size.width.0 > 0.0).then_some(bounds.size.width.0);
            if width.get_untracked() != measured {
                width.set(measured);
            }
        })
        // At the right, clear of the panels at the bottom.
        .items_end()
        .justify_center()
        .pt_safe_area()
        .p(px(40.0))
        // Clear of the play bar while it shows.
        .pb(move || {
            px(if crate::play_bar::shows(ui) {
                crate::play_bar::height(ui) + 16.0
            } else {
                40.0
            })
        })
        .test_id(if leave {
            "leave-layer"
        } else {
            "chooser-layer"
        })
        .accessibility_elements_hidden(move || overlay.accessibility_hidden(ui))
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
                .test_id(if leave {
                    "leave-panel"
                } else {
                    "chooser-panel"
                })
                // Never wider than the window (an iPad's narrowest windows).
                .max_w(percentage(1.0))
                // Nor taller than the room between the safe area and the
                // play bar: the list scrolls in what is left.
                .max_h(percentage(1.0))
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
                .bg(move || tint(ui.app.theme.with(|t| t.background.panel), 0xD0))
                .child(
                    ui_kit::heading(size, move || kind().map_or("", title))
                        .px(move || size.px(list_rows::INSET))
                        .py(move || size.px(8.0)),
                )
                .child(
                    virtual_list(count, row)
                        // A More row keeps its node when another control
                        // comes or goes around it.
                        .keys(move || {
                            let len = count();
                            ui.chooser.with(|chooser| {
                                match chooser.as_ref().and_then(|c| c.more.as_ref()) {
                                    Some(more) if len != 0 => more
                                        .controls
                                        .iter()
                                        .map(|&control| (Some(control), 0))
                                        .collect::<Vec<_>>(),
                                    _ => (0..len).map(|index| (None, index)).collect(),
                                }
                            })
                        })
                        .estimated_item_size(list_rows::pitch(size, ROW))
                        .element_ref(&list)
                        .test_id(if leave {
                            "leave-items"
                        } else {
                            "chooser-items"
                        })
                        .flex_shrink(1.0)
                        .min_h(px(0.0))
                        .h(move || {
                            let content =
                                count().clamp(1, MAX_ROWS) as f32 * list_rows::pitch(size, ROW)().0;
                            // An auxiliary window can mount before its host
                            // publishes a viewport. Never collapse its list.
                            let available = window().height.0;
                            px(if available > 0.0 {
                                content.min(available * 0.7)
                            } else {
                                content
                            })
                        }),
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
            Kind::More,
            Kind::Volume,
            Kind::Sleep,
            Kind::Track(Track::Video),
            Kind::Track(Track::Audio),
            Kind::Track(Track::Text),
            Kind::AudioDelay,
            Kind::History,
            Kind::Leave,
        ];
        let titles: BTreeSet<_> = kinds.iter().map(|k| title(*k)).collect();
        assert_eq!(titles.len(), kinds.len());
        assert_eq!(title(Kind::Track(Track::Text)), "Subtitles");
        let sizes: BTreeSet<_> = PictureSize::ALL.iter().map(|p| picture_label(*p)).collect();
        assert_eq!(sizes.len(), PictureSize::ALL.len());
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use iptv_core::tracks::{TextTrack, Tracks};

    #[test]
    fn live_track_changes_and_locale_switches_keep_open_labels_and_values_aligned() {
        let mut live = Tracks {
            text: vec![TextTrack {
                id: "original-text-track".into(),
                language: "eng".into(),
                ..Default::default()
            }],
            selected_text: Some("original-text-track".into()),
            ..Default::default()
        };
        let open = Open::for_tracks(Track::Text, live.clone());
        // A late host report replaces/reorders tracks while this chooser stays open.
        live.text = vec![TextTrack {
            id: "late-text-track".into(),
            label: "Late synthetic subtitles".into(),
            ..Default::default()
        }];
        live.selected_text = Some("late-text-track".into());
        for tag in ["es", "ja"] {
            let language = iptv_presentation::i18n::resolve(tag, None);
            let labels = open.track_labels_in(language);
            assert_eq!(
                labels[1],
                iptv_presentation::tracks::language_name_in("eng", language).unwrap()
            );
            assert_ne!(labels, entries_in(&live, Track::Text, language));
            assert_eq!(open.values[1], "original-text-track");
            assert_eq!(open.list.selected, 1);
            assert_eq!(open.list.len, labels.len());
        }
    }
}

//! Translucent choosers over full-screen video: picture size (with "apply
//! to all channels"), the sleep timer, the video, audio and subtitle
//! tracks, the audio delay, the recently watched channels (the History
//! quick action: the store's watch history, as the Recently watched list),
//! and the question before leaving where the host asks it
//! (`crate::confirm_leaving`).
//! Option sets follow the choosers observed in the reference UX;
//! wording is ours. More follows the available controls; Up/Down move,
//! OK (or a tap) picks and closes, Back closes.

mod render;
pub use render::layer;

use crate::chrome::i18n::{tr, trf};

use std::time::Duration;

use iptv_core::tracks::Kind as Track;
use iptv_presentation::player_options::{Chooser, PictureSize, SLEEP_MINUTES};
use iptv_presentation::sound::{audio_delays, delay_label};
use iptv_presentation::tracks::entries_in;
use nsh_core::Checked;
use nsh_core::prelude::*;

use crate::chrome::scale::Scale;
use crate::chrome::ui_kit;
use crate::guide::list_rows::{self, Mark, ROW};
use crate::theme::RADIUS_LARGE;
use crate::{Playback, Ui, tint};

/// Rows shown at once; longer lists scroll (seven fit a TV's 540 dp).
const MAX_ROWS: usize = 7;

pub use iptv_presentation::chooser::Kind;
use iptv_presentation::chooser::More;

pub type Open = iptv_presentation::chooser::Open<nsh_core::element::desktop::WindowRef>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum RowKey {
    More(iptv_presentation::play_bar::Control),
    Cast(String),
    Position(usize),
}

fn row_keys(open: Option<&Open>, len: usize) -> Vec<RowKey> {
    match open.filter(|_| len != 0) {
        Some(open) if open.kind == Kind::Cast => {
            open.values.iter().cloned().map(RowKey::Cast).collect()
        }
        Some(Open {
            more: Some(more), ..
        }) => more.controls.iter().copied().map(RowKey::More).collect(),
        _ => (0..len).map(RowKey::Position).collect(),
    }
}

pub(crate) fn open_more(
    ui: Ui,
    controls: Vec<iptv_presentation::play_bar::Control>,
    window: Option<nsh_core::element::desktop::WindowRef>,
) {
    if controls.is_empty() {
        return;
    }
    ui.screens.chooser.set(Some(Open {
        kind: Kind::More,
        list: Chooser::new(controls.len(), 0),
        labels: Vec::new(),
        values: Vec::new(),
        tracks: None,
        receivers: Vec::new(),
        more: Some(More { controls, window }),
    }));
}

pub(crate) fn show_receivers(ui: Ui, mut receivers: Vec<iptv_core::cast::Receiver>) {
    use iptv_core::cast::{NetworkProtocol, Target};
    let owner = crate::wiring::runtime_owner::runtime();
    if let Some((Target::Network(current), _)) = owner.cast_target()
        && !receivers.iter().any(|receiver| receiver.id == current.id)
    {
        receivers.push(current);
    }
    let mut labels = vec![tr("This device").to_owned()];
    let mut values = vec!["local".to_owned()];
    let mixed = receivers
        .iter()
        .any(|receiver| receiver.protocol == NetworkProtocol::AirPlay);
    labels.extend(receivers.iter().map(|receiver| match receiver.protocol {
        NetworkProtocol::AirPlay => trf("{name} · AirPlay", &[("name", (&receiver.name).into())]),
        NetworkProtocol::GoogleCast if mixed => {
            trf("{name} · Chromecast", &[("name", (&receiver.name).into())])
        }
        NetworkProtocol::GoogleCast => receiver.name.clone(),
    }));
    values.extend(receivers.iter().map(|r| r.id.clone()));
    // Production Apple hosts provide the real named-device control. The
    // fallback supports native ports without a mounted platform view.
    if owner.native_cast_available() && owner.native_cast_view().is_none() {
        labels.push(tr("Choose a device").to_owned());
        values.push("native".into());
    }
    let list = ui.screens.chooser.with_untracked(|open| {
        if let Some(open) = open.as_ref().filter(|open| open.kind == Kind::Cast) {
            let mut list = open.list.clone();
            list.replace_actions(&open.values, &values);
            list
        } else {
            Chooser::new(labels.len(), 0)
        }
    });
    ui.screens.chooser.set(Some(Open {
        kind: Kind::Cast,
        list,
        labels,
        values,
        tracks: None,
        receivers,
        more: None,
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
    if surface == 0 && crate::playback::on_demand::active(ui) {
        return None;
    }
    let key = ui.playback.player.stream(surface).key.get()?;
    let schedule = crate::wiring::runtime_owner::schedule();
    let channel = schedule.find_channel(&key)?;
    let identity =
        crate::wiring::runtime_owner::runtime().search_channel_identity(&*schedule, channel)?;
    Some(serde_json::json!([identity.source, identity.incarnation(), identity.content]).to_string())
}

/// The current picture size's name (the control row shows it).
pub fn picture_name(ui: Ui) -> &'static str {
    let ch = picture_identity(ui, ui.playback.player_surface);
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
        Kind::Cast => tr("Cast to TV"),
        Kind::AirPlay => tr("Cast to TV"),
        Kind::CastEnded => tr("Casting ended."),
        Kind::Leave => tr("Close Patina?"),
    }
}

fn volume_index(ui: Ui) -> usize {
    (usize::from(crate::playback::media::audible_volume(ui.playback)) + 5) / 10
}

/// The entries, what each sets, and the one in effect.
fn entries(ui: Ui, kind: Kind) -> (Vec<String>, Vec<String>, usize) {
    match kind {
        Kind::Picture => {
            let ch = picture_identity(ui, ui.playback.player_surface);
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
                .playback
                .sleep
                .with_untracked(|t| t.minutes())
                .and_then(|m| SLEEP_MINUTES.iter().position(|x| *x == m))
                .map_or(0, |i| i + 1);
            (labels, Vec::new(), at)
        }
        Kind::Track(k) => ui.playback.player.main().tracks.with_untracked(|t| {
            let labels = entries_in(t, k, crate::chrome::i18n::language());
            let values = (0..labels.len())
                .map(|i| t.pick(k, i).unwrap_or_default())
                .collect();
            (labels, values, t.selected_entry(k))
        }),
        Kind::AudioDelay => {
            let now = ui.playback.media.delay_ms.get_untracked();
            let delays = audio_delays();
            let labels = delays
                .iter()
                .map(|d| delay_label(crate::chrome::i18n::language(), *d))
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
        Kind::CastEnded => (
            vec![tr("Play here").to_owned(), tr("Close").to_owned()],
            ui.playback
                .player
                .main()
                .key
                .get_untracked()
                .into_iter()
                .collect(),
            0,
        ),
        Kind::AirPlay if ui.cast.busy.get_untracked() => {
            (vec![tr("Cancel").to_owned()], Vec::new(), 0)
        }
        Kind::AirPlay => (
            vec![tr("This device").to_owned()],
            vec!["local".to_owned()],
            0,
        ),
        Kind::Cast | Kind::More => (Vec::new(), Vec::new(), 0),
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
    let s = crate::wiring::runtime_owner::schedule();
    let now = ui.guide.focus.with_untracked(|f| f.now);
    let h = crate::guide::favorites::history(ui.playback.playing.get_untracked());
    let labels = h
        .channels
        .iter()
        .enumerate()
        .map(|(row, &ch)| {
            let p = s.program_at(ch, now);
            let marker = if h.playing == Some(row) { "  ▶" } else { "" };
            format!(
                "{}  {}{marker} · {}",
                crate::guide::lists::number(ui.guide, ch),
                s.channel_name(ch),
                crate::guide::listing_labels::listing(s.listing(ch, p))
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
    if let Kind::Track(kind) = kind {
        let tracks = ui.playback.player.main().tracks.get_untracked();
        let open = Open::for_tracks(kind, tracks, crate::chrome::i18n::language());
        if !open.labels.is_empty() {
            ui.screens.chooser.set(Some(open));
        }
        return;
    }
    let (labels, values, selected) = entries(ui, kind);
    if labels.is_empty() {
        return;
    }
    ui.screens.chooser.set(Some(Open {
        kind,
        list: Chooser::new(labels.len(), selected),
        labels,
        values,
        tracks: None,
        receivers: Vec::new(),
        more: None,
    }));
}

pub fn close(ui: Ui) {
    if ui.cast.busy.get_untracked()
        && ui
            .screens
            .chooser
            .with_untracked(|open| open.as_ref().is_some_and(|open| open.kind == Kind::AirPlay))
    {
        crate::casting::play_here(ui);
    }
    ui.screens.chooser.set(None);
}

/// Back at the top level, where the host asks before leaving: No or Yes,
/// with No focused. Always handled.
pub(crate) fn ask_to_leave(ui: Ui) -> bool {
    open(ui, Kind::Leave);
    true
}

pub fn move_focus(ui: Ui, step: isize) {
    ui.screens
        .chooser
        .update(|c| c.iter_mut().for_each(|c| c.list.move_focus(step)));
}

/// Left selects No and Right selects Yes without wrapping on repeats.
pub(crate) fn focus_leave(ui: Ui, yes: bool) {
    ui.screens.chooser.update(|open| {
        if let Some(open) = open.as_mut().filter(|open| open.kind == Kind::Leave) {
            open.list.focus = usize::from(yes);
        }
    });
}

/// OK (or a tap) on entry `index`.
pub fn pick(ui: Ui, index: usize) {
    let Some(open) = ui.screens.chooser.get_untracked() else {
        return;
    };
    let value = open.values.get(index).cloned().unwrap_or_default();
    if open.kind == Kind::Cast && value == "local" && ui.cast.busy.get_untracked() {
        crate::casting::play_here(ui);
        return;
    }
    close(ui);
    match open.kind {
        Kind::Picture => {
            let ch = picture_identity(ui, ui.playback.player_surface);
            match PictureSize::ALL.get(index) {
                Some(size) => ui
                    .app
                    .picture
                    .update(|p| p.set_channel(ch.as_deref(), *size)),
                None => ui.app.picture.update(|p| p.apply_to_all(ch.as_deref())),
            }
        }
        Kind::Sleep => match index.checked_sub(1).and_then(|i| SLEEP_MINUTES.get(i)) {
            None => ui.playback.sleep.update(|t| t.cancel()),
            Some(minutes) => start_sleep(ui, *minutes),
        },
        Kind::Track(k) if !value.is_empty() => {
            crate::playback::media::choose(ui.playback, k, value)
        }
        Kind::Track(_) => {}
        Kind::AudioDelay => {
            if let Some(ms) = audio_delays().get(index) {
                ui.playback.media.delay_ms.set(*ms);
            }
        }
        Kind::Volume => {
            if index <= 10 {
                crate::playback::media::set_volume(ui, index as i32 * 10);
            }
        }
        Kind::More => {
            if let Some(more) = open.more
                && let Some(&control) = more.controls.get(index)
            {
                crate::playback::play_bar::activate(ui, control, more.window);
            }
        }
        Kind::Cast => {
            if value == "local" {
                crate::casting::play_here(ui);
                return;
            }
            if value == "native" {
                crate::casting::select_airplay(ui);
                return;
            }
            if let Some(receiver) = open
                .receivers
                .into_iter()
                .find(|receiver| receiver.id == value)
            {
                crate::casting::select(ui, receiver);
            }
        }
        Kind::AirPlay => {
            if !ui.cast.busy.get_untracked() {
                crate::casting::play_here(ui);
            }
        }
        Kind::CastEnded => {
            if index == 0
                && ui.playback.playing.get_untracked().is_none()
                && let Some(channel) = crate::wiring::runtime_owner::schedule().find_channel(&value)
            {
                ui.play(channel, Playback::Live);
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
    let Some(channel) = crate::guide::favorites::channel_of(key) else {
        return;
    };
    crate::guide::lists::show_channel(ui, channel);
    let live = ui.playback.playback.get_untracked() == Playback::Live;
    if ui.playback.playing.get_untracked() != Some(channel) || !live {
        ui.play(channel, Playback::Live);
    }
    ui.chrome
        .mode
        .apply(crate::chrome::mode::Action::EnterFullscreen);
    ui.update_panels(|p, now| {
        p.hide();
        p.show_banner(now);
    });
}

/// Starts the sleep timer; when it fires, playback stops.
fn start_sleep(ui: Ui, minutes: u32) {
    let now = ui.now_ms();
    ui.playback.sleep.update(|t| t.set(minutes, now));
    let _task = set_timeout(Duration::from_secs(u64::from(minutes) * 60), move || {
        let mut fired = false;
        let now = ui.now_ms();
        ui.playback.sleep.update(|t| fired = t.fire_if_due(now));
        if fired {
            log::info!("iptv sleep timer: stopping playback");
            crate::playback::player_keys::stop_playback(ui);
        }
    });
}

/// Whether a chooser is open in the layer of the question before leaving
/// (`leave`, over every screen) or in the one of the others (over the
/// video, under the play bar).
pub(crate) fn is_open(ui: Ui, leave: bool) -> bool {
    ui.screens
        .chooser
        .with(|c| c.as_ref().is_some_and(|c| (c.kind == Kind::Leave) == leave))
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
        let open = Open::for_tracks(Track::Text, live.clone(), crate::chrome::i18n::language());
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

#[cfg(test)]
mod row_tests {
    use super::*;

    fn cast_rows(values: &[&str]) -> Open {
        Open {
            kind: Kind::Cast,
            labels: Vec::new(),
            values: values.iter().map(|id| (*id).to_owned()).collect(),
            tracks: None,
            receivers: Vec::new(),
            more: None,
            list: Chooser::new(values.len(), 0),
        }
    }

    #[test]
    fn inserting_a_discovered_route_preserves_the_existing_actions() {
        let before = cast_rows(&["local", "generated:known", "native"]);
        let after = cast_rows(&["local", "generated:new", "generated:known", "native"]);
        let before_keys = row_keys(Some(&before), before.list.len);
        let after_keys = row_keys(Some(&after), after.list.len);
        assert_eq!(
            before_keys,
            [
                after_keys[0].clone(),
                after_keys[2].clone(),
                after_keys[3].clone()
            ]
        );
        assert_ne!(before_keys[1], after_keys[1]);
        assert!(row_keys(Some(&after), 0).is_empty());
    }
}

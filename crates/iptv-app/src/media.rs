//! What the open streams offer and how they sound: each surface's tracks
//! and format as the `tv.player` host reports them (`tracks` and `format`
//! events, kept in its [`crate::player::Stream`]), the viewer's track
//! choices, and the app's volume, mute and audio delay (props). A new
//! stream forgets the old one's tracks and choices; the sound settings
//! stay. `video.rs` binds these to the hosts; the panels and choosers read
//! and change the full-screen player's (surface 0).

use iptv_core::player_info::StreamFormat;
use iptv_core::sound::{VOLUME_STEP, Volume};
use iptv_core::tracks::{Choice, Kind, Tracks};
use nsh_core::prelude::*;

use crate::Ui;

/// Sound state beside [`Ui`]'s (signals are `Copy`), the same for every
/// surface; multiview mutes all but its active one.
#[derive(Clone, Copy)]
pub struct Media {
    pub volume: Signal<Volume>,
    /// Audio delay in milliseconds (positive: sound later).
    pub delay_ms: Signal<i32>,
}

impl Media {
    pub fn new() -> Self {
        Media {
            volume: signal(Volume::default()),
            delay_ms: signal(0),
        }
    }
}

/// Whether this host can delay the sound (`audioDelay`): mpv on the
/// desktop, Media3 through the host's delayed audio sink.
pub const AUDIO_DELAY: bool = true;

/// `surface` opened another stream: its tracks are not known yet and the
/// choices made for the last one do not apply.
pub fn reset(ui: Ui, surface: usize) {
    let m = ui.player.stream(surface);
    if m.tracks.with_untracked(|t| *t != Tracks::default()) {
        m.tracks.set(Tracks::default());
    }
    if m.choice.with_untracked(|c| *c != Choice::default()) {
        m.choice.set(Choice::default());
    }
    if m.format.with_untracked(|f| *f != StreamFormat::default()) {
        m.format.set(StreamFormat::default());
    }
}

/// The viewer picked `value` (a track id, `auto` or `off`) for `kind` of
/// the full-screen player.
pub fn choose(ui: Ui, kind: Kind, value: String) {
    log::info!("iptv tracks: {kind:?} → {value}");
    ui.player.main().choice.update(|c| match kind {
        Kind::Video => c.video = value,
        Kind::Audio => c.audio = value,
        Kind::Text => c.text = value,
    });
}

pub fn step_volume(ui: Ui, up: bool) {
    let delta = if up { VOLUME_STEP } else { -VOLUME_STEP };
    ui.media.volume.update(|v| v.step(delta));
    log_volume(ui);
}

pub fn toggle_mute(ui: Ui) {
    ui.media.volume.update(Volume::toggle_mute);
    log_volume(ui);
}

fn log_volume(ui: Ui) {
    let v = ui.media.volume.get_untracked();
    log::info!(
        "iptv volume: {}%{}",
        v.level,
        if v.muted { ", muted" } else { "" }
    );
}

/// The host's `tracks` event from `surface`.
pub fn on_tracks(ui: Ui, surface: usize, tracks: Tracks) {
    log::info!(
        "iptv tracks {surface}: {} video, {} audio, {} subtitles",
        tracks.video.len(),
        tracks.audio.len(),
        tracks.text.len()
    );
    let m = ui.player.stream(surface);
    if m.tracks.with_untracked(|t| *t != tracks) {
        m.tracks.set(tracks);
    }
}

/// The host's `format` event from `surface`: whichever of `fps` and
/// `channels` it knows.
pub fn on_format(ui: Ui, surface: usize, fps: Option<f64>, channels: Option<u32>) {
    let m = ui.player.stream(surface);
    let mut f = m.format.get_untracked();
    if let Some(fps) = fps {
        f.fps = fps as f32;
    }
    if let Some(channels) = channels {
        f.channels = channels;
    }
    if m.format.with_untracked(|old| *old != f) {
        m.format.set(f);
    }
}

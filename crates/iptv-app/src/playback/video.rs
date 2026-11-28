//! The persistent video layer: one `tv.player` native component (registered by
//! the Android host, `hosts/android`, and on Linux by `mpv`), laid out per view mode — the preview
//! rectangle in the guide's header, or the whole screen under an overlay or under
//! nothing. Changing modes changes layout only; the player is never recreated.
//! What it plays comes from `player.rs` (resolution, retries); its tracks,
//! format and sound from `media.rs`.
//!
//! Multiview (`multiview.rs`) lays surface 0 out in its tile and mounts one
//! more `tv.player` per further tile (surfaces 1–3), each made when its
//! tile comes and dropped when it goes; all but the active tile's get
//! `muted`, and a paused tile's `paused`. A surface 0 without a tile
//! shrinks to nothing (it is never hidden or detached).
//!
//! The `tv.player` contract (both hosts; every instance is independent: its
//! own player, props and events, several playing at once). Its names are
//! declared once, in `iptv-player-mpv` (`props.rs`, `events.rs`): the props
//! go out under the names generated there ([`PlayerBindings::into_wire`])
//! and the events come in as its typed [`PlayerEvent`].
//! - props `source` `{url, headers, token, live, reconnect}` or null (a new
//!   url or token reopens; the picture goes black until the new stream's
//!   first frame, except when `reconnect` says the app reopens the stream
//!   that played, or only the token changed: the last frame stays; `live`
//!   streams play on across connections the server closes where the
//!   player can bridge them, keep their last frame after an error (the app
//!   reconnects them) and are buffered to ride out short outages; its
//!   `User-Agent` is one of the headers, else the player's own), `paused`,
//!   `pictureSize`; `videoTrack` (`auto` or a video track id),
//!   `audioTrack` (`auto` or an audio track id), `textTrack` (`off` or a
//!   text track id) for the open stream (a new source starts again from
//!   `auto`/`auto`/`off`); `volume` (0–100; the desktop's, Android keeps
//!   the system volume), `muted`, `audioDelay` (ms, positive: sound later);
//! - events `state` {state}, `size` {width, height}, `error` {code, http?},
//!   `position` {ms}; `tracks` {video: [{id, label, width, height, fps,
//!   bitrate}], audio: [{id, label, language, channels, codec}], text:
//!   [{id, label, language}], selected: {video, audio, text}} whenever they
//!   change; `format` {fps?, channels?} for what plays.
//! - events `firstRender` {token} and `pictureLost` {token}: evidence that the
//!   current source drew a frame or released its picture. AVPlay can reopen
//!   with the same token after releasing the plane on hide. Rebuffering can
//!   leave an existing picture on the plane.
//! - picture in picture (surface 0): prop `pip` {auto, enter, window},
//!   events `pip` {supported, tv, active} and `pipAction` {action}
//!   (`pip.rs`; Android's host; the desktops' players read `window`).
//! - event `audio` {change: focusLoss|focusLossTransient|focusGain|noisy}:
//!   Android's audio focus and route, which pause and resume the main
//!   player by the core's policy (`interruption.rs`).
//!
//! In the background (`lifecycle()` Background) Android's host stops every
//! player, closing its connections, and reopens it when the app comes back
//! (live at the live edge, catch-up where it was) with no events between:
//! the app only pauses its stall checks (`player::bind`). Desktop players
//! play on in a minimized window.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use iptv_player_mpv::{
    PlayerBindings, PlayerEvent, PlayerEventError, PlayerEventKind, SourceProps,
};
use nsh_core::prelude::*;

use crate::{Ui, ViewMode};

/// What a channel of the synthetic guide (shown before any playlist is
/// imported) plays: in debug builds the mock server's test streams
/// (`tools/make-hls-fixtures.sh --serve`; three test patterns cycle by
/// channel, `/archive/*` for catch-up), else nothing.
#[cfg(debug_assertions)]
pub fn synthetic_url(channel: usize, playback: crate::Playback) -> Option<String> {
    use crate::host::MOCK_SERVER;
    Some(match playback {
        crate::Playback::Live => format!("{MOCK_SERVER}/ch{}/index.m3u8", channel % 3),
        crate::Playback::CatchUp { start, offset } => {
            let s = crate::wiring::runtime_owner::schedule();
            let p = s.program_at(channel, start);
            let utc = crate::guide::unix_of(&*s, p.start) + i64::from(offset);
            let duration = (p.end - p.start) * 60 - offset;
            format!(
                "{MOCK_SERVER}/archive/ch{}/index.m3u8?utc={utc}&duration={duration}",
                channel % 3
            )
        }
    })
}

#[cfg(not(debug_assertions))]
pub fn synthetic_url(_channel: usize, _playback: crate::Playback) -> Option<String> {
    None
}

/// The full-screen player's rectangle (x, y, width, height in px) for the
/// view mode: the preview in the guide's header, the whole window, or its
/// multiview tile.
pub fn rect(ui: Ui) -> (f32, f32, f32, f32) {
    surface_rect(ui, 0)
}

/// Surface `surface`'s rectangle; empty for a surface with nothing to show.
/// While the desktops' picture-in-picture window shows surface 0 it
/// shrinks to nothing in the main window, where a placeholder takes its
/// place (`desktop/pip_window.rs`).
pub fn surface_rect(ui: Ui, surface: usize) -> (f32, f32, f32, f32) {
    if surface == 0 && crate::playback::pip::in_window(ui.playback) {
        return (0.0, 0.0, 0.0, 0.0);
    }
    layout_rect(ui, surface)
}

/// Where the view mode puts surface `surface`.
pub fn layout_rect(ui: Ui, surface: usize) -> (f32, f32, f32, f32) {
    // Android's picture-in-picture window: surface 0 fills it (`pip.rs`).
    if crate::playback::pip::fills_app(ui.playback) {
        let w = window();
        return match surface {
            0 => (0.0, 0.0, w.width.0, w.height.0),
            _ => (0.0, 0.0, 0.0, 0.0),
        };
    }
    if crate::playback::multiview::is_open(ui.playback) {
        return crate::playback::multiview::surface_rect(ui.playback, surface)
            .unwrap_or((0.0, 0.0, 0.0, 0.0));
    }
    if surface != 0 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let w = window();
    if ui.chrome.mode.get() != ViewMode::Guide {
        crate::screens::channels_view::preview_rect(ui.playback, w.width.0, w.height.0)
            .unwrap_or((0.0, 0.0, w.width.0, w.height.0))
    } else {
        crate::playback::panels::preview_rect()
    }
}

/// Each surface's opens: `source` values handed to its player with a new
/// url or token (what makes a host open a stream).
static OPENS: [AtomicU64; crate::playback::player::SURFACES] =
    [const { AtomicU64::new(0) }; crate::playback::player::SURFACES];

/// How many streams surface `surface`'s player was told to open so far
/// (for tests: one change of channel must open it once).
pub fn opens(surface: usize) -> u64 {
    OPENS.get(surface).map_or(0, |n| n.load(Ordering::Relaxed))
}

/// Whether surface `surface`'s player has a stream to play now (for
/// tests: stopping playback takes it away).
pub fn has_source(surface: usize) -> bool {
    SOURCED
        .get(surface)
        .is_some_and(|s| s.load(Ordering::Relaxed))
}

static SOURCED: [AtomicBool; crate::playback::player::SURFACES] =
    [const { AtomicBool::new(false) }; crate::playback::player::SURFACES];

/// Counts `source` as an open when its url or token differs from `last`'s.
fn count_open(surface: usize, last: &RefCell<Option<(String, u64)>>, source: Option<&SourceProps>) {
    let now = source.map(|s| (s.url.clone(), s.token));
    let mut last = last.borrow_mut();
    if now.is_some() && *last != now {
        OPENS[surface].fetch_add(1, Ordering::Relaxed);
    }
    SOURCED[surface].store(now.is_some(), Ordering::Relaxed);
    *last = now;
}

/// Whether the backdrops under the video layer leave its rectangle
/// transparent (`panels::backdrop`): on a platform whose video plane is
/// under the page, while surface 0 shows a picture (reactive). That is
/// while it plays a source whose first frame its player drew and whose
/// picture is still on the plane ([`Stream::rendered`](crate::playback::player::Stream::rendered),
/// for the open token). The backdrop covers the plane until the next frame
/// after the player releases the picture.
pub(crate) fn backdrop_clear(ui: Ui) -> bool {
    if !crate::host::video_under_page() {
        return false;
    }
    let stream = ui.playback.player.main();
    plays(ui, 0)
        && stream.source.with(Option::is_some)
        && stream.rendered.get() == Some(stream.token.get())
}

/// Whether surface `surface` has something to play (reactive).
fn plays(ui: Ui, surface: usize) -> bool {
    if surface == 0 {
        ui.playback.playing.get().is_some()
            || crate::playback::player::resume::playing(ui.playback)
            || ui.playback.player.recorded.with(Option::is_some)
            || ui.playback.player.vod.with(Option::is_some)
    } else {
        ui.playback.player.stream(surface).channel.get().is_some()
    }
}

/// The stream as the host's `source` prop: `{url, headers, token, live,
/// reconnect}`, or null when nothing plays.
fn source_prop(ui: Ui, surface: usize) -> Option<SourceProps> {
    let stream = ui.playback.player.stream(surface);
    if !plays(ui, surface) {
        return None;
    }
    let token = stream.token.get();
    // Set with the token (`player::load`).
    let live = stream.live.get_untracked();
    let reconnect = stream.reconnect.get_untracked();
    stream.source.with(|s| {
        s.as_ref().map(|s| SourceProps {
            url: s.url.clone(),
            public_only: s.public_only,
            headers: s.headers.iter().cloned().collect(),
            token,
            live,
            reconnect,
        })
    })
}

/// Player surface `surface` (0: the full-screen one).
pub fn layer(ui: Ui, surface: usize) -> impl IntoElement {
    let stream = ui.playback.player.stream(surface);
    let sent = RefCell::new(None);
    let props = PlayerBindings {
        source: Box::new(move || {
            let source = source_prop(ui, surface);
            count_open(surface, &sent, source.as_ref());
            source
        }),
        paused: Box::new(move || {
            (surface == 0 && ui.playback.paused.get())
                || crate::playback::multiview::paused(ui, surface)
        }),
        seek: Box::new(move || stream.seek.get()),
        picture_size: Box::new(move || {
            let identity = crate::chrome::chooser::picture_identity(ui.playback, surface);
            ui.app
                .picture
                .with(|p| p.for_channel(identity.as_deref()))
                .key()
                .to_owned()
        }),
        video_track: Box::new(move || stream.choice.with(|c| c.video.clone())),
        audio_track: Box::new(move || stream.choice.with(|c| c.audio.clone())),
        text_track: Box::new(move || stream.choice.with(|c| c.text.clone())),
        volume: Box::new(move || {
            ui.playback
                .player_windows
                .with(|w| w.on_surface(surface).map(|s| s.volume.level))
                .unwrap_or_else(|| ui.playback.media.volume.with(|v| v.level))
        }),
        muted: Box::new(move || {
            ui.playback
                .player_windows
                .with(|w| w.on_surface(surface).map(|s| s.volume.muted))
                .unwrap_or_else(|| ui.playback.media.volume.with(|v| v.muted))
                || !crate::playback::multiview::audible(ui, surface)
        }),
        audio_delay: Box::new(move || {
            ui.playback
                .player_windows
                .with(|w| w.on_surface(surface).map(|s| s.audio_delay))
                .unwrap_or_else(|| ui.playback.media.delay_ms.get())
        }),
        pip: Box::new(move || crate::playback::pip::prop(ui, surface)),
    };
    let mut player = native(iptv_player_mpv::PLAYER_VIEW)
        .test_id("video-player")
        .absolute()
        .left(move || px(surface_rect(ui, surface).0))
        .top(move || px(surface_rect(ui, surface).1))
        .w(move || px(surface_rect(ui, surface).2))
        .h(move || px(surface_rect(ui, surface).3));
    // Desktop prototype routing metadata; the shared player wire stays unchanged.
    if crate::host::PLAYER_POPUPS {
        player = player.prop("popupSurface", move || serde_json::json!(surface));
    }
    // Every prop of the contract, under its one declared name.
    for (prop, value) in props.into_wire() {
        player = player.prop(prop.wire(), value);
    }
    player.on_native_event(move |e: &NativeEvent| {
        #[cfg(feature = "test-support")]
        if e.name.as_ref() == "test.parental.lock" {
            crate::wiring::runtime_owner::runtime().parental_lock();
            return;
        }
        match PlayerEvent::decode(&e.name, &e.data) {
            Ok(event) => on_event(ui, surface, event),
            Err(error) => on_unread(ui.playback, surface, error),
        }
    })
}

/// An event the contract does not have, from surface `surface`'s player. It
/// does what it did before the events were typed: a state without one of
/// the four names shows on the status line and changes nothing else, a
/// `size` without its two numbers still marks reported size (the
/// stream's size stays), and the rest sets nothing.
fn on_unread(playback: crate::PlaybackUi, surface: usize, error: PlayerEventError) {
    if !playback.player.stream(surface).accepts_native(None) {
        return;
    }
    let (kind, status) = match error {
        // Another component's event or a later host's, and a `position`
        // without its `ms`: not logged.
        PlayerEventError::UnknownName
        | PlayerEventError::NoPosition
        | PlayerEventError::NoRenderToken => return,
        // The window has no such button: logged as every action is.
        PlayerEventError::UnknownAction(action) => {
            log::info!("iptv pip: {action}");
            return;
        }
        // A later host's change: nothing paused or resumed.
        PlayerEventError::UnknownAudio(change) => {
            log::info!("iptv audio: {change}");
            return;
        }
        PlayerEventError::UnknownState(name) => (PlayerEventKind::State, name),
        PlayerEventError::NoSize { width, height } => {
            if surface == 0 {
                crate::chrome::startup::mark("video size");
            }
            (PlayerEventKind::Size, format!("{width}×{height}"))
        }
    };
    report(playback, surface, kind, status);
}

/// Logs `status`, what the `kind` event from `surface` said, and shows
/// surface 0's on the status line.
fn report(playback: crate::PlaybackUi, surface: usize, kind: PlayerEventKind, status: String) {
    log::info!("iptv player {surface} {}: {status}", kind.wire());
    if surface == 0 {
        playback.player_status.set(status);
    }
}

/// One event from surface `surface`'s player.
fn on_event(ui: Ui, surface: usize, event: PlayerEvent) {
    let stream = ui.playback.player.stream(surface);
    let accepts = match &event {
        // Replacement preparation withdraws callback admission while the old
        // source stays mounted. It can still lose its picture in that window.
        PlayerEvent::PictureLost { token } => *token == stream.token.get_untracked(),
        PlayerEvent::Reauthorize { token, .. }
        | PlayerEvent::Timeline { token, .. }
        | PlayerEvent::FirstRender { token } => stream.accepts_native(Some(*token)),
        PlayerEvent::Pip { .. } | PlayerEvent::PipAction(_) | PlayerEvent::Audio(_) => true,
        _ => stream.accepts_native(None),
    };
    if !accepts {
        return;
    }
    let kind = event.kind();
    let status = match event {
        // Once a second while playing: not logged.
        PlayerEvent::Reauthorize { allowed, .. } => {
            crate::playback::player::reauthorize(ui, surface, allowed);
            return;
        }
        PlayerEvent::Position { ms } => {
            stream.position_ms.set(ms);
            crate::playback::player::watch_time::observe(ui, surface, ms);
            return;
        }
        PlayerEvent::Timeline {
            token,
            seek_id,
            ms,
            duration_ms,
            seekable,
        } => {
            crate::playback::on_demand::observe(
                ui,
                surface,
                token,
                seek_id,
                ms,
                duration_ms,
                seekable,
            );
            return;
        }
        PlayerEvent::Tracks(tracks) => {
            return crate::playback::media::on_tracks(ui.playback, surface, tracks);
        }
        PlayerEvent::Format { fps, channels } => {
            return crate::playback::media::on_format(ui.playback, surface, fps, channels);
        }
        PlayerEvent::Pip {
            supported,
            tv,
            active,
        } => return crate::playback::pip::on_state(ui, supported, tv, active),
        PlayerEvent::PipAction(action) => return crate::playback::pip::on_action(ui, action),
        PlayerEvent::Audio(change) => {
            return crate::playback::interruption::on_change(ui, surface, change);
        }
        PlayerEvent::FirstRender { token } => {
            #[cfg(feature = "test-support")]
            crate::wiring::performance_probe::rendered(ui, surface, token);
            if token == stream.token.get_untracked() {
                if surface == 0 {
                    crate::chrome::startup::mark("video rendered");
                }
                if crate::host::video_under_page() {
                    stream.rendered.set(Some(token));
                }
            }
            return;
        }
        PlayerEvent::PictureLost { token } => {
            if stream.rendered.get_untracked() == Some(token) {
                stream.rendered.set(None);
            }
            return;
        }
        PlayerEvent::State(state) => {
            crate::playback::player::on_state(ui, surface, state);
            state.name().to_owned()
        }
        PlayerEvent::Size { width, height } => {
            stream.size.set(Some((width as u32, height as u32)));
            if surface == 0 {
                crate::chrome::startup::mark("video size");
            }
            format!("{width}×{height}")
        }
        PlayerEvent::Error { code, http } => {
            // A refused source (`headersUnsupported`) or a failed one: no
            // picture until a new open draws its first frame.
            if crate::host::video_under_page() {
                stream.rendered.set(None);
            }
            crate::playback::player::on_error(ui, surface, &code, http);
            format!("error {code}")
        }
    };
    report(ui.playback, surface, kind, status);
}

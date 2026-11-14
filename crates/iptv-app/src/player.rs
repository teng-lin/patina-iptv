//! The playback driver: resolves the playing channel to a stream (off the
//! UI thread when the provider needs a round trip), feeds it to its player
//! surface (`video.rs`), and runs the core's health machine per surface:
//! retry with backoff for live streams, stall detection, the error and
//! waiting overlays (`player_panels::status`, `multiview_view`). Also
//! previous channel, back to live, and the last channel on startup.
//!
//! Surface 0 is the persistent player of the guide's preview and full
//! screen; it plays [`Ui::playing`] as [`Ui::playback`] says. Multiview
//! (`multiview.rs`) adds surfaces 1 to [`SURFACES`] − 1, each playing its
//! [`Stream::channel`] live; every surface has its own [`Stream`]: stream,
//! retries, position, size and tracks.
//!
//! The last channel starts at a cold start as soon as the store is open,
//! before the guide is read (`resume.rs`); else (a Stalker channel) after
//! the startup refresh of a stored source shows the guide (never under the
//! first-run wizard). Each playlist has its own: a
//! one-line file, `last_channel-<source id>` in [`crate::data::data_dir`],
//! holding the channel's content key (never a list index); builds before
//! that kept one `last_channel` file, still read when a playlist has none.
//! It moves to the store's history once the store keeps one for playback.

use std::rc::Rc;
use std::time::Duration;

use iptv_core::playback::{ChannelHistory, ErrorKind, Health, PlaySource, Retry};
use iptv_core::player_info::StreamFormat as Format;
use iptv_core::tracks::{Choice, Tracks};
use iptv_guide::EpgSchedule;
use iptv_guide::playback::{Clock, Resolution, StalkerLink, Unavailable};
use nsh_core::platform::Lifecycle;
use nsh_core::prelude::*;

use crate::guide::Schedule;
use crate::{Playback, Ui, ViewMode};

mod recover;
pub(crate) mod resume;

use recover::{fail, watch_stall};
pub use recover::{failed, on_error, on_state, retry, retry_surface, surface_failed};

/// Player surfaces: the full-screen one and multiview's others.
pub const SURFACES: usize = iptv_core::multiview::MAX_SURFACES;

/// One player surface's playback (signals are `Copy`).
#[derive(Clone, Copy)]
pub struct Stream {
    pub health: Signal<Health>,
    /// What the player has open (`None` until resolved).
    pub source: Signal<Option<PlaySource>>,
    /// Bumped on every open, so a retry reopens even the same URL.
    pub token: Signal<u64>,
    /// The open stream is live (not catch-up): the player bridges
    /// connections the server closes.
    pub live: Signal<bool>,
    /// The open is a retry of a stream that played: the player keeps its
    /// last frame instead of going black.
    pub reconnect: Signal<bool>,
    /// The playing channel's key (live or catch-up): finds it again in a
    /// new schedule.
    pub key: Signal<Option<String>>,
    /// What surfaces other than 0 play (catalog), always live; surface 0
    /// plays [`Ui::playing`].
    pub channel: Signal<Option<usize>>,
    /// The video's size, as the player reported it.
    pub size: Signal<Option<(u32, u32)>>,
    /// Where the open stream is (ms from where it was opened), as the
    /// player last reported.
    pub position_ms: Signal<u64>,
    /// A retry that came due while the app was away ([`recover::resume`]).
    parked: Signal<Option<Retry>>,
    /// What the open stream offers and what the viewer chose (`media.rs`).
    pub tracks: Signal<Tracks>,
    pub choice: Signal<Choice>,
    /// Frame rate and audio channels; the size is [`Stream::size`].
    pub format: Signal<Format>,
}

impl Stream {
    fn new() -> Self {
        Stream {
            health: signal(Health::default()),
            source: signal(None),
            token: signal(0),
            live: signal(true),
            reconnect: signal(false),
            key: signal(None),
            channel: signal(None),
            size: signal(None),
            position_ms: signal(0),
            parked: signal(None),
            tracks: signal(Tracks::default()),
            choice: signal(Choice::default()),
            format: signal(Format::default()),
        }
    }
}

/// Playback state beside [`Ui`]'s `playing`/`playback` (signals are `Copy`).
#[derive(Clone, Copy)]
pub struct Player {
    /// By surface; 0 is the full-screen player.
    pub streams: [Stream; SURFACES],
    pub history: Signal<ChannelHistory>,
    /// The last channel still starts on its own until something plays.
    resume: Signal<bool>,
    /// What surface 0 plays before the guide arrived (`resume.rs`).
    early: Signal<Option<resume::Early>>,
    /// The app's owner: retry and stall timers outlive the effect or
    /// handler that armed them.
    owner: Owner,
}

impl Player {
    pub fn new() -> Self {
        Self {
            streams: std::array::from_fn(|_| Stream::new()),
            history: signal(ChannelHistory::default()),
            resume: signal(true),
            early: signal(None),
            owner: Owner::current(),
        }
    }

    /// The full-screen player's (surface 0).
    pub fn main(&self) -> Stream {
        self.streams[0]
    }

    pub fn stream(&self, surface: usize) -> Stream {
        self.streams[surface.min(SURFACES - 1)]
    }
}

/// What `surface` plays: surface 0 the playing channel as `playback`
/// says, the others their channel live.
pub fn target(ui: Ui, surface: usize) -> Option<(usize, Playback)> {
    if surface == 0 {
        Some((ui.playing.get_untracked()?, ui.playback.get_untracked()))
    } else {
        let channel = ui.player.stream(surface).channel.get_untracked()?;
        Some((channel, Playback::Live))
    }
}

/// The playback context of an imported source: what its streams need from
/// it (headers, account, session). Attach it with
/// `EpgSchedule::with_playback`.
pub use crate::runtime::playback::stored_context;

/// `f` after `delay`, under the app's owner.
fn later(ui: Ui, delay: Duration, f: impl FnOnce() + 'static) {
    ui.player.owner.run(|| {
        let _task = set_timeout(delay, f);
    });
}

/// The viewer's clock, with the local UTC offset at `at` (the programme's
/// start for catch-up: date placeholders describe that instant).
fn clock(at: Option<i64>) -> Clock {
    let now = crate::time::wall_unix();
    Clock {
        now,
        local_offset_secs: crate::time::offset_minutes(at.unwrap_or(now)) * 60,
    }
}

fn real_schedule(s: &Rc<dyn Schedule>) -> Option<&EpgSchedule> {
    s.as_any()?.downcast_ref::<EpgSchedule>()
}

/// Called by [`Ui::play`] after it set `channel` playing and the playback:
/// opens the stream with a fresh health session.
pub fn start(ui: &Ui, channel: usize) {
    let ui = *ui;
    let live = ui.playback.get_untracked() == Playback::Live;
    let now = ui.now_ms();
    let mut session = 0;
    let main = ui.player.main();
    main.health.update(|h| session = h.start(live, now));
    ui.player.resume.set(false);
    let key = crate::schedule().channel_key(channel);
    main.key.set(key.clone());
    if live && let Some(key) = key {
        let changed = ui
            .player
            .history
            .with_untracked(|h| h.current() != Some(&key));
        ui.player.history.update(|h| h.played(&key));
        if changed {
            if crate::data::combined::active() {
                if let Some((source, local)) = crate::data::combined::owner(&key) {
                    save_last_channel(Some(source), &local);
                }
                crate::runtime::playback::save_combined_channel(&key);
            } else {
                save_last_channel(ui.library.shown.get_untracked(), &key);
            }
            crate::favorites::played(&key);
        }
    }
    open(ui, 0, session);
}

/// Surface `surface` (not 0) plays `channel` live, with a fresh health
/// session; the channel history is the full-screen player's only.
pub fn start_surface(ui: Ui, surface: usize, channel: usize) {
    if surface == 0 {
        ui.play(channel, Playback::Live);
        return;
    }
    let stream = ui.player.stream(surface);
    let now = ui.now_ms();
    let mut session = 0;
    stream.health.update(|h| session = h.start(true, now));
    stream.channel.set(Some(channel));
    stream.key.set(crate::schedule().channel_key(channel));
    log::info!("iptv player {surface}: starting");
    open(ui, surface, session);
}

/// Nothing plays: the player closes.
pub fn stop(ui: Ui) {
    close(ui, 0);
}

/// Surface `surface` plays nothing (surface 0 keeps its channel's key;
/// [`stop`] is its own).
pub fn stop_surface(ui: Ui, surface: usize) {
    if surface == 0 {
        ui.playing.set(None);
        stop(ui);
        return;
    }
    let stream = ui.player.stream(surface);
    stream.channel.set(None);
    stream.key.set(None);
    close(ui, surface);
}

fn close(ui: Ui, surface: usize) {
    cancel_resolve(surface);
    let stream = ui.player.stream(surface);
    stream.health.update(Health::stop);
    stream.parked.set(None);
    stream.source.set(None);
    if stream.size.with_untracked(Option::is_some) {
        stream.size.set(None);
    }
    crate::media::reset(ui, surface);
}

/// Resolves what `surface` plays and hands the stream to its player.
fn open(ui: Ui, surface: usize, session: u64) {
    // What the surface asked for before is no longer wanted.
    cancel_resolve(surface);
    let Some((channel, playback)) = target(ui, surface) else {
        // A retry of the last channel started before the guide arrived.
        match resume::source(ui).filter(|_| surface == 0) {
            Some(source) => load(ui, 0, session, source),
            None => close(ui, surface),
        }
        return;
    };
    let schedule = crate::schedule();
    // An older day's programme: its real times once its day is paged in.
    if let Playback::CatchUp { start, .. } = playback
        && !crate::data::pages::ready(
            &schedule,
            channel,
            crate::guide::unix_of(&*schedule, start),
            {
                move || {
                    if ui
                        .player
                        .stream(surface)
                        .health
                        .with_untracked(|h| h.session())
                        == session
                    {
                        open(ui, surface, session);
                    }
                }
            },
        )
    {
        return;
    }
    let (archive, offset) = match playback {
        Playback::Live => (None, 0),
        Playback::CatchUp { start, offset } => (Some(schedule.program_at(channel, start)), offset),
    };
    let combined = schedule
        .as_any()
        .and_then(|s| s.downcast_ref::<iptv_guide::combined::CombinedSchedule>());
    if real_schedule(&schedule).is_none() && combined.is_none() {
        // The synthetic guide plays the local test streams (debug builds).
        match crate::video::synthetic_url(channel, playback) {
            Some(url) => load(ui, surface, session, PlaySource::new(url)),
            None => fail(
                ui,
                surface,
                session,
                ErrorKind::Unavailable,
                "no stream".into(),
            ),
        }
        return;
    };
    let at = archive.map(|p| crate::guide::unix_of(&*schedule, p.start));
    let resolution = match combined {
        Some(s) => s.resolve_from(channel, archive, i64::from(offset), clock(at)),
        None => real_schedule(&schedule).unwrap().resolve_from(
            channel,
            archive,
            i64::from(offset),
            clock(at),
        ),
    };
    match resolution {
        Resolution::Play(source) => load(ui, surface, session, source),
        Resolution::StalkerLink(link) => resolve_link(ui, surface, session, link, schedule.clone()),
        Resolution::Unavailable(why) => {
            let detail = match why {
                Unavailable::NoChannel => "no channel".to_owned(),
                Unavailable::NeedsAccount => "no account".to_owned(),
                Unavailable::NoArchive(e) => format!("{e:?}"),
            };
            fail(ui, surface, session, ErrorKind::Unavailable, detail);
        }
    }
}

fn load(ui: Ui, surface: usize, session: u64, mut source: PlaySource) {
    let stream = ui.player.stream(surface);
    if stream.health.with_untracked(|h| h.session()) != session {
        return; // the viewer moved on
    }
    crate::settings::add_user_agent(&mut source.headers, crate::settings::user_agent());
    let (live, reconnect) = stream
        .health
        .with_untracked(|h| (h.live(), h.reconnecting()));
    if surface == 0 {
        crate::startup::mark("player opened");
    }
    log::info!(
        "iptv player {surface} open: session {session}, {} header(s){}",
        source.headers.len(),
        if reconnect { ", reconnecting" } else { "" }
    );
    // One write for the player: the `source` prop must never see the new
    // token with the old url (a host would open the old stream again
    // first, as leaving multiview did).
    batch(|| {
        stream.live.set(live);
        stream.reconnect.set(reconnect);
        stream.token.update(|t| *t += 1);
        stream.position_ms.set(0);
        // A new stream: its own tracks, from the default choices.
        crate::media::reset(ui, surface);
        stream.source.set(Some(source));
    });
    let now = ui.now_ms();
    stream.health.update(|h| h.resolved(now));
    watch_stall(ui, surface);
}

/// Cancels `surface`'s link request in flight, if any: the viewer zapped,
/// the surface reopens or plays nothing.
fn cancel_resolve(surface: usize) {
    crate::runtime().cancel_link_resolution(surface);
}

/// A Stalker `create_link` on the shared network runtime (`executor.rs`);
/// the result comes back to the UI thread. Opening anything else on the
/// surface, or closing it, cancels the request (`cancel_resolve`).
fn resolve_link(
    ui: Ui,
    surface: usize,
    session: u64,
    link: Box<StalkerLink>,
    schedule: Rc<dyn Schedule>,
) {
    let context = target(ui, surface)
        .and_then(|(channel, _)| iptv_guide::combined::epg_channel(&*schedule, channel))
        .and_then(|(epg, _)| epg.shared_playback());
    let StalkerLink {
        session: mut portal,
        cmd,
        channel_headers,
    } = *link;
    let cancel = crate::runtime().begin_link_resolution(surface, session);
    let client = crate::runtime::playback::link_client(crate::settings::net_config());
    let request = crate::executor::spawn(async move {
        let result = match client.await {
            Ok(net) => iptv_import::stalker_link(&net, &mut portal, &cmd, false, &cancel)
                .await
                .map_err(|e| format!("{:?}", e.kind())),
            Err(_) => Err("network".to_owned()),
        };
        (result, portal)
    });
    let _task = ui.player.owner.run(|| {
        spawn(crate::in_runtime(async move {
            let answered = request.await;
            crate::runtime().finish_link_resolution(surface, session);
            let Ok((result, portal)) = answered else {
                return;
            };
            if ui
                .player
                .stream(surface)
                .health
                .with_untracked(|h| h.session())
                != session
            {
                return;
            }
            // This resolve still owns the player's session. Organization copies
            // share its playback context even though their schedule pointers differ.
            if let Some(ctx) = context {
                ctx.update_stalker_session(portal.clone());
            }
            match result {
                Ok(url) => load(
                    ui,
                    surface,
                    session,
                    StalkerLink::source(&portal, url, &channel_headers),
                ),
                Err(kind) => fail(ui, surface, session, ErrorKind::from_resolve(&kind), kind),
            }
        }))
    });
}

/// Plays the channel before the current one (again: back).
pub fn previous_channel(ui: Ui) {
    let Some(key) = ui
        .player
        .history
        .with_untracked(|h| h.previous().map(str::to_owned))
    else {
        return;
    };
    let Some(channel) = crate::schedule().find_channel(&key) else {
        return;
    };
    show_channel(ui, channel);
    ui.play(channel, Playback::Live);
    if ui.mode.get_untracked() == ViewMode::Fullscreen
        && crate::settings::flag(iptv_settings::keys::INFO_ON_SWITCH)
    {
        ui.show_playing();
    }
}

/// From catch-up back to the live stream of the playing channel.
pub fn go_live(ui: Ui) {
    if let (Some(channel), Playback::CatchUp { .. }) =
        (ui.playing.get_untracked(), ui.playback.get_untracked())
    {
        ui.play(channel, Playback::Live);
        if ui.mode.get_untracked() == ViewMode::Fullscreen {
            ui.show_playing();
        }
    }
}

fn show_channel(ui: Ui, channel: usize) {
    crate::lists::show_channel(ui, channel);
}

pub use crate::runtime::playback::forget_source;
/// The last channel's file of playlist `source` (the shared one of older
/// builds for none).
use crate::runtime::playback::{load_last_channel, save_last_channel};

/// Another playlist is about to be shown (`data.rs`): what plays and the
/// channel history belong to the old one (channel keys are unique only
/// within a playlist, so the new one may reuse the playing key).
pub fn source_changed(ui: Ui) {
    crate::multiview::close(ui);
    if ui.playing.get_untracked().is_some() {
        log::info!("iptv player: another playlist is shown; stopping");
        ui.playing.set(None);
        stop(ui);
        if ui.mode.get_untracked() != ViewMode::Guide {
            ui.mode.set(ViewMode::Guide);
        }
    }
    ui.player.main().key.set(None);
    ui.player.history.set(ChannelHistory::default());
}

/// Data changes keep the playing channel (found again by key); stall
/// checks pause while paused (full screen's pause, or a multiview tile's).
pub fn bind(ui: Ui) {
    effect(move || {
        ui.data_version.get();
        // Out of the effect: following the row writes signals.
        later(ui, Duration::ZERO, move || follow_playing(ui));
    });
    // Changes only when a tile's pause does, not on every multiview step.
    let tiles_paused = computed(move || {
        std::array::from_fn::<bool, SURFACES, _>(|s| crate::multiview::paused(ui, s))
    });
    effect(move || {
        let paused = ui.paused.get();
        let tiles_paused = tiles_paused.get();
        let away = stopped_in_background(lifecycle());
        let now = ui.now_ms();
        untracked(|| {
            for (surface, tile_paused) in tiles_paused.into_iter().enumerate() {
                let held = away || (surface == 0 && paused) || tile_paused;
                ui.player
                    .stream(surface)
                    .health
                    .update(|h| h.set_paused(held, now));
                if !held {
                    watch_stall(ui, surface);
                }
                if !away {
                    // Out of the effect: a retry opens the stream.
                    later(ui, Duration::ZERO, move || recover::resume(ui, surface));
                }
            }
        });
    });
}

/// Whether the host stops its players in `lifecycle`: Android's does while
/// the activity is stopped (`TvActivity`, `PlayerComponent.background`) and
/// reopens them after, and so does iOS's (`mpv/ios.rs`), so waiting then
/// is no stall. Desktop players play on in a minimised window.
fn stopped_in_background(lifecycle: Lifecycle) -> bool {
    cfg!(any(target_os = "android", target_os = "ios")) && lifecycle == Lifecycle::Background
}

fn follow_playing(ui: Ui) {
    if crate::multiview::follow(ui) {
        return; // multiview follows its tiles, surface 0's too
    }
    let Some(playing) = ui.playing.get_untracked() else {
        return;
    };
    let key = ui.player.main().key.get_untracked();
    match key.and_then(|k| crate::schedule().find_channel(&k)) {
        // The guide keeps its own focus by identity (`lists::restore`).
        Some(row) => {
            if row != playing {
                ui.playing.set(Some(row));
            }
        }
        // Another source (or the channel left it): its row now means
        // another channel, so nothing plays.
        None => gone(ui),
    }
}

/// The playing channel is not in the new guide: nothing plays, the guide
/// shows.
fn gone(ui: Ui) {
    log::info!("iptv player: the playing channel is not in the new guide; stopping");
    ui.playing.set(None);
    ui.player.main().key.set(None);
    stop(ui);
    if ui.mode.get_untracked() != ViewMode::Guide {
        ui.mode.set(ViewMode::Guide);
    }
}

/// `set_schedule` swapped in a guide with fewer rows than the playing
/// channel's: it is found again by key, as [`follow_playing`] finds it,
/// else nothing plays (multiview follows its tiles itself).
pub fn row_dropped(ui: Ui) {
    let key = ui.player.main().key.get_untracked();
    match key.and_then(|k| crate::schedule().find_channel(&k)) {
        Some(row) => ui.playing.set(Some(row)),
        None if crate::multiview::session(ui) => ui.playing.set(None),
        None => gone(ui),
    }
}

/// `data.rs` put an imported source's schedule on screen. After the
/// startup refresh of a stored source (`refresh`), and with no wizard open,
/// the last channel starts once (`general.resume_last_channel`), unless
/// something already plays.
pub fn source_ready(ui: Ui, refresh: bool) {
    if resume::adopt(ui) {
        return;
    }
    if !refresh
        || !crate::settings::flag(iptv_settings::keys::RESUME_LAST_CHANNEL)
        || !ui.player.resume.get_untracked()
        || ui.playing.get_untracked().is_some()
        || ui.wizard.with_untracked(Option::is_some)
    {
        return;
    }
    let last = load_last_channel(ui.library.shown.get_untracked());
    let Some(channel) = last.and_then(|k| crate::schedule().find_channel(&k)) else {
        return;
    };
    log::info!("iptv player: resuming the last channel");
    crate::startup::mark("last channel resolved");
    show_channel(ui, channel);
    ui.play(channel, Playback::Live);
    ui.mode.set(ViewMode::Fullscreen);
    ui.show_playing();
}

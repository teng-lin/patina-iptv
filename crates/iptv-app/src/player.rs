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

use crate::i18n::key;

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
pub(crate) mod watch_time;

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
    publication: Signal<iptv_core::playback::Publication>,
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
    pub timeline: Signal<iptv_core::on_demand::Timeline>,
    pub seek: Signal<Option<iptv_player_mpv::SeekProps>>,
    pub resume_ms: Signal<Option<u64>>,
    pub finished: Signal<bool>,
    /// A retry that came due while the app was away ([`recover::resume`]).
    parked: Signal<Option<Retry>>,
    /// What the open stream offers and what the viewer chose (`media.rs`).
    pub tracks: Signal<Tracks>,
    pub choice: Signal<Choice>,
    /// Frame rate and audio channels; the size is [`Stream::size`].
    pub format: Signal<Format>,
}

impl Stream {
    pub(crate) fn native_published(self) -> bool {
        self.publication.with(|publication| {
            publication.accepts(self.health.with(|health| health.session()), None)
        })
    }
    pub(crate) fn accepts_native(self, token: Option<u64>) -> bool {
        self.publication.with_untracked(|publication| {
            publication.accepts(self.health.with_untracked(|health| health.session()), token)
        })
    }
    fn new() -> Self {
        Stream {
            health: signal(Health::default()),
            source: signal(None),
            token: signal(0),
            publication: signal(Default::default()),
            live: signal(true),
            reconnect: signal(false),
            key: signal(None),
            channel: signal(None),
            size: signal(None),
            position_ms: signal(0),
            timeline: signal(Default::default()),
            seek: signal(None),
            resume_ms: signal(None),
            finished: signal(false),
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
    pub vod: Signal<Option<(iptv_runtime::vod::Playing, PlaySource)>>,
    pub recorded: Signal<Option<(iptv_core::recording::Job, PlaySource)>>,
    /// By surface; 0 is the full-screen player.
    pub streams: [Stream; SURFACES],
    pub history: Signal<ChannelHistory>,
    watching: Stored<Option<watch_time::Session>>,
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
            recorded: signal(None),
            vod: signal(None),
            streams: std::array::from_fn(|_| Stream::new()),
            history: signal(ChannelHistory::default()),
            watching: stored(None),
            resume: signal(true),
            early: signal(None),
            owner: Owner::current(),
        }
    }

    /// A fresh window presentation controlling an existing persistent surface.
    /// Only native streams and their task owner cross the window boundary;
    /// on-demand targets, history and startup-resume state remain independent.
    pub(crate) fn for_window(self, surface: usize) -> Self {
        let mut streams = self.streams;
        streams[0] = self.stream(surface);
        Self {
            vod: signal(None),
            recorded: signal(None),
            streams,
            history: signal(ChannelHistory::default()),
            watching: stored(None),
            resume: signal(false),
            early: signal(None),
            owner: self.owner,
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

/// The viewer's clock, with the local UTC offset at `at` (the program's
/// start for catch-up: date placeholders describe that instant).
fn clock(at: Option<i64>) -> Clock {
    let now = crate::time::wall_unix();
    crate::runtime::playback::clock(now, at)
}

/// Identity and UTC survive a catalog reorder or a different guide epoch.
struct PendingArchive {
    key: String,
    at: i64,
    offset: i32,
    authority: Option<iptv_guide::playback::ResolutionAuthority>,
}
impl PendingArchive {
    fn capture(schedule: &dyn Schedule, channel: usize, start: i32, offset: i32) -> Option<Self> {
        Some(Self {
            key: schedule.channel_key(channel)?,
            at: crate::guide::unix_of(schedule, start),
            offset,
            authority: resolution_authority(schedule, channel),
        })
    }
    fn remap(&self, schedule: &dyn Schedule) -> Option<(usize, Playback)> {
        let channel = schedule.find_channel(&self.key)?;
        let authority = resolution_authority(schedule, channel);
        match (&self.authority, &authority) {
            (Some(before), Some(after))
                if before.source() == after.source()
                    && before.incarnation() == after.incarnation()
                    && before.config() == after.config() => {}
            (None, None) => {}
            _ => return None,
        }
        let seconds = self.at.checked_sub(schedule.epoch_unix())?;
        if seconds.rem_euclid(60) != 0 {
            return None;
        }
        let start = i32::try_from(seconds.div_euclid(60)).ok()?;
        Some((
            channel,
            Playback::CatchUp {
                start,
                offset: self.offset,
            },
        ))
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
            if crate::data::combined::active(&crate::runtime()) {
                if let Some((source, local)) = crate::data::combined::owner(&crate::runtime(), &key)
                {
                    save_last_channel(Some(source), &local);
                }
                crate::runtime().save_combined_channel(&key);
            } else {
                save_last_channel(ui.app.library.shown.get_untracked(), &key);
            }
            crate::favorites::played(&key);
            ui.app.list_version.update(|v| *v += 1);
        }
    }
    open(ui, 0, session);
}

pub fn toggle_pause(ui: Ui) {
    crate::interruption::viewer_acted(ui);
    if crate::on_demand::active(ui)
        && ui.player.main().finished.get_untracked()
        && ui.player.main().accepts_native(None)
    {
        ui.paused.set(false);
        retry_surface(ui, 0);
    } else {
        ui.paused.update(|p| *p = !*p);
        crate::on_demand::flush(ui, false);
    }
}

/// Applies the guide's user-controlled playback block before any PIN check.
pub fn allow_guide_channel(channel: usize) -> bool {
    use iptv_core::organization::Block;
    match crate::schedule().channel_block(channel) {
        Block::None => true,
        Block::Channel => {
            crate::settings_view::notify(crate::i18n::tr("Unblock this channel to play it."));
            false
        }
        Block::Group => {
            crate::settings_view::notify(crate::i18n::tr(
                "Unblock the group to play this channel.",
            ));
            false
        }
    }
}

/// Applies newly published guide blocks to live and catch-up sessions.
pub fn enforce_guide_blocks(ui: Ui) {
    let schedule = crate::schedule();
    if let Some(early) = resume::source(ui)
        && early_guide_blocked(&early)
    {
        stop_surface(ui, 0);
        ui.mode.set(crate::settings::browse_mode());
    }
    let mut stopped = false;
    for surface in 0..SURFACES {
        let channel = ui
            .player
            .stream(surface)
            .key
            .get_untracked()
            .and_then(|key| schedule.find_channel(&key));
        if channel.is_some_and(|channel| {
            schedule.channel_block(channel) != iptv_core::organization::Block::None
        }) {
            stop_surface(ui, surface);
            stopped = true;
        }
    }
    if stopped {
        crate::multiview::follow(ui);
    }
}

fn early_guide_blocked(early: &resume::Early) -> bool {
    let owner = crate::runtime();
    let schedule = crate::schedule();
    let key = if crate::data::combined::active(&owner) {
        iptv_guide::combined::qualified_key(early.source_id, &early.key)
    } else if owner
        .shown_source()
        .is_some_and(|source| source.0 == early.source_id)
    {
        early.key.clone()
    } else {
        return false;
    };
    schedule.find_channel(&key).is_some_and(|channel| {
        schedule.channel_block(channel) != iptv_core::organization::Block::None
    })
}

/// Surface `surface` (not 0) plays `channel` live, with a fresh health
/// session; the channel history is the full-screen player's only.
/// Returns false when the provider connection cannot be reserved.
pub fn start_surface(ui: Ui, surface: usize, channel: usize) -> bool {
    if surface != 0
        && (!allow_guide_channel(channel)
            || !crate::parental_view::allow_play(ui, channel, Playback::Live))
    {
        return false;
    }
    if surface == 0 {
        ui.play(channel, Playback::Live);
        return ui.playing.get_untracked() == Some(channel);
    }
    if !crate::recordings_view::reserve_viewing(ui, surface, channel, true) {
        let previous = ui.player.stream(surface).channel.get_untracked();
        ui.multiview.update(|m| {
            if let Some(m) = m {
                m.retain(|s, ch| {
                    if s != surface {
                        return true;
                    }
                    if let Some(old) = previous {
                        *ch = old;
                        true
                    } else {
                        false
                    }
                });
            }
        });
        return false;
    }
    let stream = ui.player.stream(surface);
    let now = ui.now_ms();
    let mut session = 0;
    stream.health.update(|h| session = h.start(true, now));
    stream.channel.set(Some(channel));
    stream.key.set(crate::schedule().channel_key(channel));
    log::info!("iptv player {surface}: starting");
    open(ui, surface, session);
    true
}

/// Nothing plays: the player closes.
pub fn stop(ui: Ui) {
    ui.player.resume.set(false);
    ui.player.early.set(None);
    ui.pending_zap.set(None);
    ui.browse_playback.set(Default::default());
    crate::on_demand::flush(ui, false);
    crate::runtime().parental_content_closed();
    ui.player.vod.set(None);
    ui.player.recorded.set(None);
    crate::runtime().close_recording_playback();
    close(ui, 0);
}

pub fn play_recording(ui: Ui, job: iptv_core::recording::Job, source: PlaySource) {
    crate::vod_view::flush(ui);
    ui.player.vod.set(None);
    crate::multiview::close(ui);
    cancel_resolve(0);
    ui.pending_zap.set(None);
    ui.browse_playback.set(Default::default());
    ui.player.resume.set(false);
    ui.player.early.set(None);
    ui.playing.set(None);
    ui.player.main().key.set(None);
    ui.playback.set(Playback::Live);
    ui.player.recorded.set(Some((job, source.clone())));
    ui.paused.set(false);
    crate::runtime().release_viewing(0);
    let mut session = 0;
    ui.player
        .main()
        .health
        .update(|h| session = h.start(false, ui.now_ms()));
    load(ui, 0, session, source);
    ui.mode.set(ViewMode::Fullscreen);
}

/// Plays a movie or an episode on surface 0: full screen, or (`full_screen`
/// false) where the screen shows the player now, the preview behind a
/// series' details for its next episode (#187).
pub fn play_vod(
    ui: Ui,
    playing: iptv_runtime::vod::Playing,
    source: PlaySource,
    full_screen: bool,
) {
    crate::interruption::viewer_acted(ui);
    crate::on_demand::flush(ui, false);
    let resume = playing.progress.resume_ms();
    crate::multiview::close(ui);
    cancel_resolve(0);
    crate::runtime().close_recording_playback();
    ui.pending_zap.set(None);
    ui.browse_playback.set(Default::default());
    ui.player.resume.set(false);
    ui.player.early.set(None);
    ui.playing.set(None);
    ui.player.main().key.set(None);
    ui.playback.set(Playback::Live);
    ui.player.recorded.set(None);
    ui.player.vod.set(Some((playing, source.clone())));
    ui.paused.set(false);
    let mut session = 0;
    ui.player
        .main()
        .health
        .update(|h| session = h.start(false, ui.now_ms()));
    load(ui, 0, session, source);
    if resume > 0 {
        ui.player.main().resume_ms.set(Some(resume));
    }
    if full_screen {
        ui.mode.set(ViewMode::Fullscreen);
    }
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
    crate::runtime().release_viewing(surface);
    cancel_resolve(surface);
    let stream = ui.player.stream(surface);
    stream.publication.update(|publication| publication.clear());
    stream.health.update(Health::stop);
    stream.parked.set(None);
    stream.source.set(None);
    stream.timeline.set(Default::default());
    stream.seek.set(None);
    stream.resume_ms.set(None);
    if stream.size.with_untracked(Option::is_some) {
        stream.size.set(None);
    }
    crate::media::reset(ui, surface);
}

/// Resume through fresh authorization, separate from transport recovery/backoff.
pub(crate) fn reauthorize(ui: Ui, surface: usize, allowed: bool) {
    let stream = ui.player.stream(surface);
    if !allowed {
        stop_surface(ui, surface);
        crate::settings_view::notify(crate::i18n::tr(
            "Provider connection reserved for recording",
        ));
        return;
    }
    let mut session = None;
    stream
        .health
        .update(|health| session = health.reauthorize(ui.now_ms()));
    if let Some(session) = session {
        stream.parked.set(None);
        if surface == 0 {
            recover::resume_position(ui);
        }
        open(ui, surface, session);
    }
}

/// Resolves what `surface` plays and hands the stream to its player.
fn open(ui: Ui, surface: usize, session: u64) {
    let stream = ui.player.stream(surface);
    if stream.health.with_untracked(|health| health.session()) != session {
        return;
    }
    stream.publication.update(|publication| publication.clear());
    if surface == 0
        && let Some((playing, _)) = ui.player.vod.get_untracked()
    {
        let position = if ui.player.main().finished.get_untracked() {
            0
        } else {
            ui.player
                .main()
                .resume_ms
                .get_untracked()
                .unwrap_or_else(|| ui.player.main().timeline.get_untracked().position_ms)
        };
        let key = playing.key.clone();
        crate::data::spawn_in_app(async move {
            let owner = crate::runtime();
            let resolved = owner.vod_play(playing.details, key.clone(), false).await;
            if ui.player.main().health.with_untracked(|h| h.session()) != session
                || !ui
                    .player
                    .vod
                    .with_untracked(|p| p.as_ref().is_some_and(|(p, _)| p.key == key))
            {
                return;
            }
            match resolved {
                Ok((mut playing, source))
                    if owner.reserve_viewing(0, iptv_library::SourceId(key.source)) =>
                {
                    playing.progress.position_ms = position;
                    ui.player.vod.set(Some((playing, source.clone())));
                    load(ui, 0, session, source);
                    if position > 0 {
                        ui.player.main().resume_ms.set(Some(position));
                    }
                }
                Ok(_) => fail(
                    ui,
                    0,
                    session,
                    ErrorKind::Http(429),
                    crate::i18n::tr("Provider connection reserved for recording").into(),
                ),
                Err(_) => {
                    let allowed = ui.player.vod.with_untracked(|p| {
                        p.as_ref().is_some_and(|(p, _)| {
                            owner.parental_allowed(&iptv_runtime::vod::target(&p.key))
                                && p.parent.as_ref().is_none_or(|key| {
                                    owner.parental_allowed(&iptv_runtime::vod::target(key))
                                })
                        })
                    });
                    if allowed {
                        fail(
                            ui,
                            0,
                            session,
                            ErrorKind::Resolve,
                            crate::i18n::tr("On-demand playback unavailable").into(),
                        );
                    } else {
                        stop_surface(ui, 0);
                    }
                }
            }
        });
        return;
    }

    if let Some((channel, _)) = target(ui, surface)
        && let Some(target) = crate::parental_view::live_target(ui, channel)
    {
        crate::data::spawn_in_app(async move {
            let owner = crate::runtime();
            let allowed = owner
                .live_channel_allowed(iptv_library::SourceId(target.source), target.id)
                .await
                .unwrap_or(false);
            if ui
                .player
                .stream(surface)
                .health
                .with_untracked(|h| h.session())
                != session
            {
                return;
            }
            if allowed {
                open_allowed(ui, surface, session);
            } else {
                stop_surface(ui, surface);
            }
        });
    } else {
        open_allowed(ui, surface, session);
    }
}
fn open_allowed(ui: Ui, surface: usize, session: u64) {
    if surface == 0
        && let Some((job, source)) = ui.player.recorded.get_untracked()
    {
        let position = if ui.player.main().finished.get_untracked() {
            0
        } else {
            ui.player
                .main()
                .resume_ms
                .get_untracked()
                .unwrap_or_else(|| ui.player.main().timeline.get_untracked().position_ms)
        };
        let owner = crate::runtime();
        crate::data::spawn_in_app(async move {
            let target = iptv_core::parental::Target::live(job.request.source, job.request.channel);
            let allowed = owner.parental_check(&target).await.is_ok();
            if ui.player.main().health.with_untracked(|h| h.session()) != session {
                return;
            }
            if allowed {
                load(ui, surface, session, source);
                ui.player.main().resume_ms.set(Some(position));
            } else {
                stop(ui);
            }
        });
        return;
    }
    // What the surface asked for before is no longer wanted.
    cancel_resolve(surface);
    let Some((channel, playback)) = target(ui, surface) else {
        // A retry of the last channel started before the guide arrived.
        match resume::source(ui).filter(|_| surface == 0) {
            Some(early) => crate::data::spawn_in_app(async move {
                let owner = crate::runtime();
                let source = iptv_library::SourceId(early.source_id);
                let channel = iptv_providers::ContentId::from_stored(early.key.clone());
                let allowed = crate::runtime::commands::channel(&owner, source, channel)
                    .await
                    .is_ok_and(|row| row.is_some_and(|r| !r.blocked));
                if ui.player.main().health.with_untracked(|h| h.session()) != session {
                    return;
                }
                if !allowed {
                    stop(ui);
                    return;
                }
                if owner.reserve_live_viewing(0, source, &early.key) {
                    load_authorized(ui, 0, session, early.source, Some(early.authority));
                } else {
                    fail(
                        ui,
                        0,
                        session,
                        ErrorKind::Http(429),
                        crate::i18n::tr("Provider connection reserved for recording").into(),
                    );
                }
            }),
            None => close(ui, surface),
        }
        return;
    };
    if !crate::recordings_view::try_reserve_viewing(
        surface,
        channel,
        matches!(playback, Playback::Live),
    ) {
        fail(
            ui,
            surface,
            session,
            ErrorKind::Http(429),
            crate::i18n::tr("Provider connection reserved for recording").into(),
        );
        return;
    }
    let schedule = crate::schedule();
    // An older day's program: capture semantic identity and UTC before
    // waiting. ScheduleReady can synchronously complete this waiter before
    // the deferred follow_playing effect has remapped the numeric row.
    if let Playback::CatchUp { start, offset } = playback {
        let pending = PendingArchive::capture(&*schedule, channel, start, offset);
        if !crate::data::pages::ready(
            &crate::runtime(),
            &schedule,
            channel,
            crate::guide::unix_of(&*schedule, start),
            move |result| {
                if ui
                    .player
                    .stream(surface)
                    .health
                    .with_untracked(|h| h.session())
                    != session
                    || (surface == 0 && ui.playback.get_untracked() != playback)
                {
                    return;
                }
                let target = result
                    .ok()
                    .and_then(|()| pending.and_then(|pending| pending.remap(&*crate::schedule())));
                if let Some((channel, playback)) = target {
                    batch(|| {
                        if surface == 0 {
                            ui.playing.set(Some(channel));
                            ui.playback.set(playback);
                        } else {
                            ui.player.stream(surface).channel.set(Some(channel));
                        }
                    });
                    open(ui, surface, session);
                } else {
                    fail(
                        ui,
                        surface,
                        session,
                        ErrorKind::Unavailable,
                        crate::i18n::tr("Program guide could not be loaded").into(),
                    );
                }
            },
        ) {
            return;
        }
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
                key("no stream").into(),
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
    let authority = resolution_authority(&*schedule, channel);
    match resolution {
        Resolution::Play(source) => load_authorized(ui, surface, session, source, authority),
        Resolution::StalkerLink(link) => {
            resolve_link(ui, surface, session, link, schedule.clone(), authority)
        }
        Resolution::Unavailable(why) => {
            let detail = match why {
                Unavailable::NoChannel => key("no channel").to_owned(),
                Unavailable::NeedsAccount => key("no account").to_owned(),
                Unavailable::NoArchive(e) => format!("{e:?}"),
            };
            fail(ui, surface, session, ErrorKind::Unavailable, detail);
        }
    }
}

fn source_allowed(ui: Ui, surface: usize) -> bool {
    let owner = crate::runtime();
    if surface == 0
        && let Some((playing, _)) = ui.player.vod.get_untracked()
    {
        owner.parental_allowed(&iptv_runtime::vod::target(&playing.key))
            && playing
                .parent
                .as_ref()
                .is_none_or(|p| owner.parental_allowed(&iptv_runtime::vod::target(p)))
    } else if surface == 0
        && let Some((job, _)) = ui.player.recorded.get_untracked()
    {
        owner.parental_allowed(&iptv_core::parental::Target::live(
            job.request.source,
            job.request.channel,
        ))
    } else if let Some((channel, _)) = target(ui, surface) {
        crate::schedule().channel_block(channel) == iptv_core::organization::Block::None
            && crate::parental_view::live_target(ui, channel)
                .is_none_or(|t| owner.parental_allowed(&t))
    } else if let Some(early) = resume::source(ui) {
        !early_guide_blocked(&early)
            && owner.parental_allowed(&iptv_core::parental::Target::live(
                early.source_id,
                early.key,
            ))
    } else {
        true
    }
}

/// Local admission uses the same retryable refusal as a connection held by
/// recording in open_allowed(). The channel resolved; its address is present.
fn fail_capacity(ui: Ui, surface: usize, session: u64, error: &crate::runtime::Error) -> bool {
    let reason = crate::i18n::key("No provider connection available; check Streams at once");
    if error.unavailable_reason() != Some(reason) {
        return false;
    }
    fail(
        ui,
        surface,
        session,
        ErrorKind::Http(429),
        crate::i18n::tr("No provider connection available; check Streams at once").into(),
    );
    true
}

fn resolution_authority(
    schedule: &dyn Schedule,
    channel: usize,
) -> Option<iptv_guide::playback::ResolutionAuthority> {
    let (schedule, channel) = iptv_guide::combined::epg_channel(schedule, channel)?;
    schedule.resolution_authority(channel)
}

fn load(ui: Ui, surface: usize, session: u64, source: PlaySource) {
    let authority = target(ui, surface)
        .and_then(|(channel, _)| resolution_authority(&*crate::schedule(), channel))
        .or_else(|| resume::source(ui).map(|early| early.authority));
    load_authorized(ui, surface, session, source, authority);
}

fn load_authorized(
    ui: Ui,
    surface: usize,
    session: u64,
    mut source: PlaySource,
    authority: Option<iptv_guide::playback::ResolutionAuthority>,
) {
    let stream = ui.player.stream(surface);
    if stream.health.with_untracked(|h| h.session()) != session {
        return; // the viewer moved on
    }
    let may_probe = stream
        .publication
        .with_untracked(|publication| publication.may_probe_replacement());
    stream.publication.update(|publication| publication.clear());
    // Recheck timed grants after asynchronous resolution, before the decoder
    // sees a stream. The runtime already checked current durable membership.
    let owner = crate::runtime();
    let allowed = source_allowed(ui, surface);
    if !allowed {
        stop_surface(ui, surface);
        return;
    }

    crate::settings::add_user_agent(&mut source.headers, crate::settings::user_agent());
    // Callers set a resume request immediately after load(). Preparation may
    // finish later, so clear the previous session before that request is set.
    stream.resume_ms.set(None);
    let identity = if stream.health.with_untracked(|h| h.live()) {
        target(ui, surface)
            .and_then(|(channel, _)| {
                crate::schedule()
                    .channel_key(channel)
                    .and_then(|key| crate::data::combined::owner(&owner, &key))
            })
            .or_else(|| {
                resume::source(ui).map(|early| (iptv_library::SourceId(early.source_id), early.key))
            })
    } else {
        None
    };
    if let Some((source_id, channel)) = identity.clone().filter(|_| may_probe) {
        let Some(authority) = authority else {
            fail(
                ui,
                surface,
                session,
                ErrorKind::Unavailable,
                crate::i18n::tr("Channel unavailable").into(),
            );
            return;
        };
        let prepared = owner.prepare_live_source(surface, source_id, channel, source, authority);
        crate::data::spawn_in_app(async move {
            match prepared.await {
                Ok(source) => publish_source(ui, surface, session, source),
                Err(error) => {
                    if !fail_capacity(ui, surface, session, &error) {
                        fail(
                            ui,
                            surface,
                            session,
                            ErrorKind::Unavailable,
                            crate::i18n::tr("Channel unavailable").into(),
                        );
                    }
                }
            }
        });
    } else {
        let vod_authority = if surface == 0
            && let Some((playing, _)) = ui.player.vod.get_untracked()
        {
            match owner.vod_publication_authority(&playing) {
                Ok(authority) => Some(owner.validate_vod_publication(authority)),
                Err(_) => {
                    stop_surface(ui, surface);
                    return;
                }
            }
        } else {
            None
        };
        let provider = identity.or_else(|| {
            if surface == 0
                && let Some((playing, _)) = ui.player.vod.get_untracked()
            {
                Some((
                    iptv_library::SourceId(playing.key.source),
                    format!("vod:{:?}:{}", playing.key.kind, playing.key.id),
                ))
            } else if ui.player.recorded.with_untracked(Option::is_none) {
                target(ui, surface)
                    .and_then(|(channel, _)| crate::schedule().channel_key(channel))
                    .and_then(|key| crate::data::combined::owner(&owner, &key))
            } else {
                None
            }
        });
        let prepared = owner.prepare_independent_source(surface, provider, source, authority);
        crate::data::spawn_in_app(async move {
            let source = match prepared.await {
                Ok(source) => source,
                Err(error) => {
                    if !fail_capacity(ui, surface, session, &error)
                        && stream.health.with_untracked(|health| health.session()) == session
                    {
                        stop_surface(ui, surface);
                    }
                    return;
                }
            };
            if let Some(authority) = vod_authority
                && !authority.await
            {
                if stream.health.with_untracked(|health| health.session()) == session {
                    stop_surface(ui, surface);
                }
                return;
            }
            publish_source(ui, surface, session, source);
        });
    }
}

fn publish_source(ui: Ui, surface: usize, session: u64, source: PlaySource) {
    let stream = ui.player.stream(surface);
    if stream.health.with_untracked(|h| h.session()) != session {
        return;
    }
    if !source_allowed(ui, surface) {
        stop_surface(ui, surface);
        return;
    }
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
    let ownership = if surface == 0 && ui.player.recorded.with_untracked(Option::is_some) {
        iptv_core::playback::SourceOwnership::Local
    } else if crate::runtime().owns_viewing_source(surface, &source) {
        iptv_core::playback::SourceOwnership::RuntimeProvider
    } else {
        iptv_core::playback::SourceOwnership::NativeProvider
    };
    // One write for the player: the `source` prop must never see the new
    // token with the old url (a host would open the old stream again
    // first, as leaving multiview did).
    batch(|| {
        stream.live.set(live);
        stream.reconnect.set(reconnect);
        stream.token.update(|t| *t += 1);
        stream.publication.update(|publication| {
            publication.publish(session, stream.token.get_untracked(), ownership)
        });
        stream.position_ms.set(0);
        stream.finished.set(false);
        stream.timeline.set(Default::default());
        stream.seek.set(None);
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
    authority: Option<iptv_guide::playback::ResolutionAuthority>,
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
    let client = crate::runtime().link_client(crate::settings::network_config());
    let request = crate::runtime().spawn_network(async move {
        let result = match client.await {
            Ok(net) => iptv_import::stalker_link(&net, &mut portal, &cmd, false, &cancel)
                .await
                .map_err(|e| match e {
                    // `ErrorKind::from_resolve`'s name for a build without
                    // Stalker portals.
                    iptv_import::ImportError::Unsupported(_) => "Unsupported".to_owned(),
                    e => format!("{:?}", e.kind()),
                }),
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
                Ok(url) => load_authorized(
                    ui,
                    surface,
                    session,
                    StalkerLink::source(&portal, url, &channel_headers),
                    authority,
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

/// Playlist `source` was deleted: its last channel goes too (after any
/// write still queued for it).
pub fn forget_source(source: crate::runtime::model::SourceId) {
    crate::runtime().forget_last_channel(source);
}
/// The last channel of playlist `source` (the shared one of older builds
/// for none), a write still queued included.
fn load_last_channel(source: Option<crate::runtime::model::SourceId>) -> Option<String> {
    crate::runtime().load_last_channel(source)
}
/// Queues the write: a zap never waits for the disk.
fn save_last_channel(source: Option<crate::runtime::model::SourceId>, key: &str) {
    crate::runtime().save_last_channel(source, key);
}

/// Another playlist is about to be shown (`data.rs`): what plays and the
/// channel history belong to the old one (channel keys are unique only
/// within a playlist, so the new one may reuse the playing key).
pub fn source_changed(ui: Ui) {
    crate::desktop::player_windows::close_all(ui);
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
    cleanup(move || crate::on_demand::flush(ui, false));
    effect(move || {
        ui.app.data_version.get();
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
            if paused || away {
                crate::on_demand::flush(ui, false);
            }
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
/// is no stall. Desktop players play on in a minimized window.
fn stopped_in_background(lifecycle: Lifecycle) -> bool {
    (crate::host::ANDROID || crate::host::IPAD) && lifecycle == Lifecycle::Background
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
    if crate::desktop::recorder_only_startup()
        || ui.player.recorded.with_untracked(Option::is_some)
        || ui.player.vod.with_untracked(Option::is_some)
    {
        return;
    }
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
    let last = load_last_channel(ui.app.library.shown.get_untracked());
    let Some(channel) = last.and_then(|k| crate::schedule().find_channel(&k)) else {
        return;
    };
    if crate::schedule().channel_block(channel) != iptv_core::organization::Block::None {
        return;
    }
    log::info!("iptv player: resuming the last channel");
    crate::startup::mark("last channel resolved");
    // The guide's list follows only playback the parental lock admitted
    // (at once, or after its PIN): a canceled PIN leaves the guide as it
    // was.
    crate::parental_view::admitted(ui, channel, move || {
        ui.play(channel, Playback::Live);
        if ui.playing.get_untracked() != Some(channel) {
            return;
        }
        crate::lists::show_resumed(ui, channel);
        ui.mode.set(ViewMode::Fullscreen);
        ui.show_playing();
    });
}

#[cfg(test)]
mod pending_archive_regressions {
    use super::*;
    use iptv_providers::{
        SecretUrl,
        model::{M3uSourceConfig, SourceConfig},
    };

    fn guide(reversed: bool, epoch: i64, incarnation: &str) -> EpgSchedule {
        let parsed = iptv_providers::m3u::parse_m3u(concat!(
            "#EXTM3U\n",
            "#EXTINF:-1 tvg-id=\"generated-one\" catchup=\"shift\" catchup-days=\"3\",Generated one\n",
            "https://example.test/one.ts\n",
            "#EXTINF:-1 tvg-id=\"generated-two\" catchup=\"shift\" catchup-days=\"3\",Generated two\n",
            "https://example.test/two.ts\n",
        ));
        let mut channels = parsed.channels.items;
        if reversed {
            channels.reverse();
        }
        let authority = iptv_guide::playback::SourceAuthority::new(
            7,
            incarnation.into(),
            SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("https://example.test/generated.m3u"),
                user_agent: None,
                referrer: None,
            }),
        );
        EpgSchedule::from_catalog(
            &parsed.groups,
            &channels,
            iptv_epg::GuideStore::default(),
            epoch,
        )
        .with_source_authority(Some(std::sync::Arc::new(authority)))
    }

    #[test]
    fn pending_catchup_remaps_identity_and_utc_before_replacement_retry() {
        let before = guide(false, 1_800_000_000, "generated-original");
        let pending = PendingArchive::capture(&before, 0, -1800, 73).unwrap();
        // Simulate the successful waiter running synchronously on ScheduleReady,
        // while the UI still holds old row 0 and minute -1800.
        let after = guide(true, before.epoch_unix() + 86400, "generated-original");
        assert_eq!(before.channel_count(), after.channel_count());
        assert_ne!(before.channel_key(0), after.channel_key(0));
        let (row, playback) = pending.remap(&after).unwrap();
        assert_eq!(row, 1);
        assert_eq!(before.channel_key(0), after.channel_key(row));
        let Playback::CatchUp { start, offset } = playback else {
            panic!("catch-up retry");
        };
        assert_eq!(start, -1800 - 1440);
        assert_eq!(offset, 73);
        assert_eq!(
            crate::guide::unix_of(&after, start),
            crate::guide::unix_of(&before, -1800)
        );
        // Matching content/numeric source IDs cannot bind to a recreated source.
        let recreated = guide(true, after.epoch_unix(), "generated-replacement");
        assert!(pending.remap(&recreated).is_none());
        let removed = iptv_core::guide::SyntheticSchedule;
        assert!(pending.remap(&removed).is_none());
    }
}

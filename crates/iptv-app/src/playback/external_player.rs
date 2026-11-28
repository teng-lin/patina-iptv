//! Handoff for an explicit channel choice in a group with external playback.
use crate::runtime::Runtime;
use crate::runtime::model::{ResolveFailure, StreamPurpose};
use crate::{Playback, Ui, ViewMode, chrome::i18n::tr, host};
use iptv_core::guide::{Program, Schedule};
use iptv_core::parental::Target;
use iptv_guide::playback::ResolutionAuthority;
use iptv_presentation::external_player::{Handoff, State};
use iptv_presentation::lists::{Entry, ListId};
use nsh_core::prelude::{Memo, Signal, Stored, computed, signal, stored, untracked};
use std::rc::Rc;

#[derive(Clone, Copy)]
pub(crate) struct ExternalUi {
    state: Signal<State>,
    pub(crate) request: Memo<u64>,
    session: Stored<Option<Transport>>,
}

impl ExternalUi {
    pub(crate) fn new() -> Self {
        let state = signal(State::default());
        Self {
            state,
            request: computed(move || state.with(State::request)),
            session: stored(None),
        }
    }

    fn active(self) -> Option<Active> {
        let handoff = self.state.with_untracked(|state| state.active().cloned())?;
        let transport = self.session.get()?;
        Some(Active { handoff, transport })
    }

    fn activate(self, handoff: Handoff, transport: Transport) -> bool {
        if self.request.get_untracked() != handoff.ticket {
            return false;
        }
        self.session.replace(Some(transport));
        self.state.update(|state| state.activate(handoff));
        true
    }
}

#[derive(Clone)]
struct Active {
    handoff: Handoff,
    transport: Transport,
}

#[derive(Clone)]
struct Transport {
    authority: ResolutionAuthority,
    group: String,
    session: host::ExternalSession,
}

pub fn enforce(cast: crate::screens::casting::CastUi) {
    let Some(active) = cast.external.active() else {
        return;
    };
    if active.transport.session.is_retired() {
        clear_retired(cast, &active.handoff);
        return;
    }
    if !active.transport.session.is_open() {
        return;
    }
    if !crate::wiring::runtime_owner::runtime().parental_allowed(&active.handoff.target) {
        active.transport.session.close();
        return;
    }
    let owner = crate::wiring::runtime_owner::runtime();
    crate::wiring::tasks::spawn_in_app(async move {
        let allowed = owner
            .external_playback_allowed(active.transport.authority, active.transport.group)
            .await;
        if !matches!(allowed, Ok(true)) && active_ticket(cast, active.handoff.ticket) {
            active.transport.session.close();
        }
    });
}

fn active_ticket(cast: crate::screens::casting::CastUi, ticket: u64) -> bool {
    cast.external
        .state
        .with_untracked(|state| state.is_active(ticket))
}

fn clear_retired(cast: crate::screens::casting::CastUi, handoff: &Handoff) {
    if active_ticket(cast, handoff.ticket) {
        cast.external.session.replace(None);
        cast.external
            .state
            .update(|state| state.clear_active(handoff.ticket));
        if pending_target(cast).as_ref() != Some(&handoff.target) {
            crate::wiring::runtime_owner::runtime().parental_end_transfer(&handoff.target);
        }
    }
}

pub(crate) fn pending_target(cast: crate::screens::casting::CastUi) -> Option<Target> {
    cast.external
        .state
        .with_untracked(|state| state.pending_target().cloned())
}

pub(crate) fn handoff_target(ui: Ui) -> Option<Target> {
    pending_target(ui.cast).or_else(|| {
        ui.cast
            .external
            .active()
            .filter(|active| active.transport.session.is_open())
            .map(|active| active.handoff.target)
    })
}

pub(crate) fn is_playing(cast: crate::screens::casting::CastUi) -> bool {
    cast.external.session.with(|transport| {
        transport
            .as_ref()
            .is_some_and(|transport| transport.session.is_open())
    })
}

pub(crate) fn returned(cast: crate::screens::casting::CastUi) {
    stop(cast);
    host::external_player_returned();
}

pub(crate) fn stop(cast: crate::screens::casting::CastUi) {
    cancel_pending(cast);
    if let Some(active) = cast.external.active() {
        active.transport.session.close();
        crate::wiring::runtime_owner::runtime().parental_end_transfer(&active.handoff.target);
    }
}

fn clear_pending_target(cast: crate::screens::casting::CastUi, ticket: Option<u64>) {
    let external = cast.external;
    let Some(mut state) = untracked(|| external.state.try_get()) else {
        return;
    };
    let Some(target) = state.clear_pending(ticket) else {
        return;
    };
    external.state.set(state);
    if !external
        .active()
        .is_some_and(|active| active.handoff.target == target && active.transport.session.is_open())
    {
        crate::wiring::runtime_owner::runtime().parental_end_transfer(&target);
    }
}

struct PendingHandoff {
    ui: Ui,
    ticket: u64,
    owner: Rc<crate::runtime::Runtime>,
}

impl PendingHandoff {
    fn new(ui: Ui, ticket: u64, owner: Rc<crate::runtime::Runtime>) -> Self {
        Self { ui, ticket, owner }
    }

    fn mark(&self, target: Target) {
        self.ui
            .cast
            .external
            .state
            .update(|state| state.mark_pending(self.ticket, target));
    }

    fn return_to_internal(&self, target: &Target) {
        if pending_target(self.ui.cast).as_ref() == Some(target) {
            // The existing per-use grant follows the same content back to
            // the native player. Ui::play still checks its ordinary gates.
            self.ui.cast.external.state.update(|state| {
                state.clear_pending(Some(self.ticket));
            });
        }
    }
}

impl Drop for PendingHandoff {
    fn drop(&mut self) {
        crate::wiring::runtime_owner::with_runtime(self.owner.clone(), || {
            clear_pending_target(self.ui.cast, Some(self.ticket))
        });
    }
}

struct PreviousPlayback {
    key: String,
    authority: ResolutionAuthority,
    archive: Option<crate::playback::player::PendingArchive>,
    mode: ViewMode,
}

impl PreviousPlayback {
    fn capture(ui: Ui) -> Option<Self> {
        let channel = ui.playback.playing.get_untracked()?;
        let schedule = crate::wiring::runtime_owner::schedule();
        let authority = Runtime::playback_authority(&*schedule, channel)?;
        let archive = match ui.playback.playback.get_untracked() {
            Playback::Live => None,
            Playback::CatchUp { start, offset } => {
                Some(crate::playback::player::PendingArchive::capture(
                    &*schedule, channel, start, offset,
                )?)
            }
        };
        Some(Self {
            key: schedule.channel_key(channel)?,
            authority,
            archive,
            mode: ui.chrome.mode.get_untracked(),
        })
    }

    fn remap(&self, schedule: &dyn Schedule) -> Option<(usize, Playback)> {
        let channel = schedule.find_channel(&self.key)?;
        if Runtime::playback_authority(schedule, channel).as_ref() != Some(&self.authority) {
            return None;
        }
        match &self.archive {
            Some(archive) => archive.remap(schedule),
            None => Some((channel, Playback::Live)),
        }
    }

    async fn restore(self, pending: &PendingHandoff, current: impl Fn() -> bool) {
        let source = iptv_library::SourceId(self.authority.source());
        let channel = self.authority.channel().id.as_str();
        if !matches!(
            pending
                .owner
                .live_channel_allowed(source, channel.to_owned())
                .await,
            Ok(true)
        ) {
            return;
        }
        let ui = pending.ui;
        if !current()
            || ui.playback.playing.get_untracked().is_some()
            || crate::playback::on_demand::active(ui.playback)
            || ui.cast.external.active().is_some()
        {
            return;
        }
        let Some((channel, playback)) = self.remap(&*crate::wiring::runtime_owner::schedule())
        else {
            return;
        };
        if !crate::playback::player::allow_guide_channel(channel) {
            return;
        }
        let Some(target) = crate::screens::parental_view::live_target(ui.app, channel) else {
            return;
        };
        if !pending.owner.parental_allowed(&target) {
            return;
        }
        pending.return_to_internal(&target);
        ui.play(channel, playback);
        if ui.playback.playing.get_untracked() == Some(channel) {
            ui.chrome
                .mode
                .apply(crate::chrome::mode::Action::Restore(self.mode));
        } else {
            pending.owner.parental_end_transfer(&target);
        }
    }
}

fn group(guide: crate::GuideUi) -> Option<(ListId, usize)> {
    let schedule = crate::wiring::runtime_owner::schedule();
    let entry = guide.list.with_untracked(|list| list.entry());
    let Entry::Group(group) = Entry::at(entry) else {
        return None;
    };
    schedule
        .group_preferences(group)
        .external_player
        .then(|| (ListId::of(entry, &*schedule), group))
}

pub fn cancel_pending(cast: crate::screens::casting::CastUi) {
    crate::wiring::runtime_owner::runtime().cancel_stream_resolution(StreamPurpose::External);
    cast.external.state.update(State::supersede);
    clear_pending_target(cast, None);
    if let Some(active) = cast.external.active()
        && active.transport.session.is_retired()
    {
        clear_retired(cast, &active.handoff);
    }
}

pub fn maybe_open(ui: Ui, channel: usize, archive: Option<Program>) -> bool {
    matches!(open_from(ui, channel, archive, None), Outcome::Handled)
}

pub(crate) enum Outcome {
    Handled,
    Internal,
    MissingHandler(ListId),
}

pub(crate) fn open_archive(
    ui: Ui,
    choice: Rc<crate::playback::archive_playback::Choice>,
) -> Outcome {
    let Some((channel, program)) = choice.resolve(ui) else {
        return Outcome::Handled;
    };
    if !choice.external_context(ui) {
        return Outcome::Internal;
    }
    open_from(ui, channel, Some(program), Some(choice))
}

/// Archive fallback calls this only after authorization starts playback.
pub(crate) fn missing_handler(screens: crate::ScreensUi, list: ListId) {
    crate::chrome::menu_view::offer_external_player_reset(screens, list);
    crate::chrome::toast::notify(tr(
        "No external player is installed, so this channel plays here.",
    ));
}

fn open_from(
    ui: Ui,
    channel: usize,
    archive: Option<Program>,
    choice: Option<Rc<crate::playback::archive_playback::Choice>>,
) -> Outcome {
    if !host::EXTERNAL_PLAYER || ui.playback.player_surface != 0 {
        return Outcome::Internal;
    }
    let Some((list, group_index)) = group(ui.guide) else {
        return Outcome::Internal;
    };
    let schedule = crate::wiring::runtime_owner::schedule();
    if !schedule.group_channels(group_index).contains(&channel) {
        return if choice.is_some() {
            Outcome::Handled
        } else {
            Outcome::Internal
        };
    }
    // The player chosen for this group may have been removed since: the
    // caller plays the channel here instead, and the viewer may turn the
    // group's preference off.
    let available = host::external_player_available();
    if !available && choice.is_none() {
        missing_handler(ui.screens, list);
        return Outcome::Internal;
    }
    let owner = crate::wiring::runtime_owner::runtime();
    let Some(target) = crate::screens::parental_view::live_target(ui.app, channel) else {
        return if choice.is_some() {
            Outcome::Handled
        } else {
            Outcome::Internal
        };
    };
    let Some((epg, local)) = iptv_guide::combined::epg_channel(&*schedule, channel) else {
        return if choice.is_some() {
            Outcome::Handled
        } else {
            Outcome::Internal
        };
    };
    let Some(authority) = epg.resolution_authority(local) else {
        return if choice.is_some() {
            Outcome::Handled
        } else {
            Outcome::Internal
        };
    };
    let Some((_, group_id)) =
        crate::wiring::data::combined::owner(&owner, &schedule.group_key(group_index))
    else {
        return if choice.is_some() {
            Outcome::Handled
        } else {
            Outcome::Internal
        };
    };
    let Some(channel_key) = schedule.channel_key(channel) else {
        return if choice.is_some() {
            Outcome::Handled
        } else {
            Outcome::Internal
        };
    };
    if !available {
        return Outcome::MissingHandler(list);
    }
    if !crate::playback::player::allow_guide_channel(channel) {
        return Outcome::Handled;
    }
    let at = archive.map(|p| crate::guide::unix_of(&*schedule, p.start));
    let clock = crate::runtime::time::playback_clock(crate::guide::clock::wall_unix(), at);
    let epoch = schedule.epoch_unix();
    let epg_offset = epg.epg_offset_minutes(local);
    let expected = authority.clone();
    let guarded = choice.clone();
    let archive_choice = choice.is_some();
    let resolve = move || {
        let schedule = crate::wiring::runtime_owner::schedule();
        let channel = schedule.find_channel(&channel_key)?;
        let (epg, local) = iptv_guide::combined::epg_channel(&*schedule, channel)?;
        if epg.resolution_authority(local).as_ref() != Some(&authority)
            || (archive.is_some() && epg.epg_offset_minutes(local) != epg_offset)
        {
            return None;
        }
        let (archive, clock) = if let Some(choice) = &choice {
            let (selected, program) = choice.resolve_handoff(ui)?;
            if selected != channel {
                return None;
            }
            let clock = crate::runtime::time::playback_clock(
                crate::time::wall_unix(),
                Some(crate::guide::unix_of(&*schedule, program.start)),
            );
            let offset = epg.minute(schedule.epoch_unix());
            (
                Some(Program {
                    start: program.start + offset,
                    end: program.end + offset,
                }),
                clock,
            )
        } else {
            (
                archive.map(|p| {
                    let offset = epg.minute(epoch);
                    Program {
                        start: p.start + offset,
                        end: p.end + offset,
                    }
                }),
                clock,
            )
        };
        Some((epg.resolve(local, archive, clock), epg.shared_playback()))
    };
    cancel_pending(ui.cast);
    let ticket = ui.cast.external.request.get_untracked();
    crate::playback::browse_playback::hold(ui);
    let current = move || {
        ui.cast.external.request.get_untracked() == ticket
            && group(ui.guide).is_some_and(|(now, _)| now == list)
            && guarded
                .as_ref()
                .is_none_or(|choice| choice.resolve_handoff(ui).is_some())
    };
    let before_prompt = current.clone();
    let protected = target.clone();
    let admitted = move || {
        if !current() {
            return;
        }
        let Some((resolution, context)) = resolve() else {
            unavailable();
            return;
        };
        let owner = crate::wiring::runtime_owner::runtime();
        crate::wiring::tasks::spawn_in_app(async move {
            let source = match owner
                .resolve_stream(StreamPurpose::External, resolution, context)
                .await
            {
                Ok(source) => source,
                Err(ResolveFailure::Cancelled) => return,
                Err(_) => {
                    if current() {
                        unavailable();
                    }
                    return;
                }
            };
            if !current() {
                return;
            }
            connect_handoff(
                ui, owner, ticket, target, expected, group_id, source, current, resolve,
            )
            .await;
        });
    };
    crate::wiring::navigation::request_if(
        ui.screens,
        protected,
        move || !archive_choice || before_prompt(),
        admitted,
    );
    Outcome::Handled
}

#[allow(clippy::too_many_arguments)]
async fn connect_handoff(
    ui: Ui,
    owner: Rc<Runtime>,
    ticket: u64,
    target: Target,
    expected: ResolutionAuthority,
    group_id: String,
    mut source: iptv_core::playback::PlaySource,
    current: impl Fn() -> bool,
    resolve: impl Fn() -> Option<(
        iptv_guide::playback::Resolution,
        Option<Rc<iptv_guide::playback::PlaybackContext>>,
    )>,
) {
    owner.apply_user_agent(&mut source.headers);
    let pending = PendingHandoff::new(ui, ticket, owner.clone());
    let handoff = host::connect_external_player(source, || {
        if current() {
            pending.mark(target.clone());
            true
        } else {
            false
        }
    })
    .await;
    if !current() {
        return;
    }
    let Some(handoff) = handoff else {
        unavailable();
        return;
    };
    let verified = owner
        .external_playback_allowed(expected.clone(), group_id.clone())
        .await;
    if !current() {
        return;
    }
    if !handoff.is_open()
        || !matches!(verified, Ok(true))
        || !owner.parental_allowed(&target)
        || resolve().is_none()
    {
        unavailable();
        return;
    }
    if let Some(previous) = ui.cast.external.active() {
        previous.transport.session.close_and_wait().await;
        clear_retired(ui.cast, &previous.handoff);
        if !current() {
            return;
        }
        if !handoff.is_open() {
            unavailable();
            return;
        }
    }
    let Some(lease) = owner.reserve_external_viewing(iptv_library::SourceId(target.source)) else {
        crate::chrome::toast::notify(tr(
            "No provider connection available while recording; stop a recording or check Streams at once",
        ));
        return;
    };
    let previous = PreviousPlayback::capture(ui);
    crate::playback::interruption::viewer_acted(ui.playback);
    crate::playback::player_keys::stopped_for_external(ui, &target);
    owner.retire_viewing_inputs(0).await;
    if !current() {
        return;
    }
    let approval = owner
        .authorize_external_playback(expected.clone(), group_id.clone())
        .await;
    if !current() {
        return;
    }
    let session = match (handoff, lease) {
        (handoff, lease)
            if handoff.is_open()
                && owner.parental_allowed(&target)
                && resolve().is_some()
                && matches!(approval, Ok(Some(approval)) if approval.is_current()) =>
        {
            handoff.allow(lease)
        }
        // Release the handoff and lease before attempting recovery.
        _ => None,
    };
    if let Some(session) = session {
        let transport = Transport {
            authority: expected,
            group: group_id,
            session,
        };
        let handoff = Handoff { ticket, target };
        if !ui
            .cast
            .external
            .activate(handoff.clone(), transport.clone())
        {
            transport.session.close();
            return;
        }
        crate::wiring::tasks::spawn_in_app(async move {
            transport.session.wait_retired().await;
            clear_retired(ui.cast, &handoff);
        });
    } else {
        unavailable();
        if let Some(previous) = previous {
            previous.restore(&pending, current).await;
        }
    }
}

fn unavailable() {
    crate::chrome::toast::notify(tr("Could not open this channel in an external player."));
}

//! Handoff for an explicit channel choice in a group with external playback.
use crate::{Playback, Ui, ViewMode, host, i18n::tr};
use iptv_core::guide::{Program, Schedule};
use iptv_core::parental::Target;
use iptv_guide::playback::{Resolution, ResolutionAuthority, StalkerLink};
use iptv_presentation::lists::{Entry, ListId};
use std::rc::Rc;

#[derive(Clone)]
pub(crate) struct Active {
    ticket: u64,
    target: Target,
    authority: ResolutionAuthority,
    group: String,
    session: host::ExternalSession,
}

impl PartialEq for Active {
    fn eq(&self, other: &Self) -> bool {
        self.ticket == other.ticket
    }
}

pub fn enforce(ui: Ui) {
    let Some(active) = ui.external_playback.get_untracked() else {
        return;
    };
    if active.session.is_retired() {
        clear_retired(ui, &active);
        return;
    }
    if !active.session.is_open() {
        return;
    }
    if !crate::runtime_owner::runtime().parental_allowed(&active.target) {
        active.session.close();
        return;
    }
    let owner = crate::runtime_owner::runtime();
    crate::tasks::spawn_in_app(async move {
        let allowed = owner
            .external_playback_allowed(active.authority, active.group)
            .await;
        if !matches!(allowed, Ok(true)) && active_ticket(ui, active.ticket) {
            active.session.close();
        }
    });
}

fn active_ticket(ui: Ui, ticket: u64) -> bool {
    ui.external_playback
        .with_untracked(|state| state.as_ref().is_some_and(|state| state.ticket == ticket))
}

fn clear_retired(ui: Ui, active: &Active) {
    if active_ticket(ui, active.ticket) {
        ui.external_playback.set(None);
        if pending_target(ui).as_ref() != Some(&active.target) {
            crate::runtime_owner::runtime().parental_end_transfer(&active.target);
        }
    }
}

fn pending_target(ui: Ui) -> Option<Target> {
    let (ticket, target) = ui.external_handoff_target.get_untracked()?;
    (ui.external_request.get_untracked() == ticket).then_some(target)
}

pub(crate) fn handoff_target(ui: Ui) -> Option<Target> {
    pending_target(ui).or_else(|| {
        ui.external_playback
            .get_untracked()
            .filter(|active| active.session.is_open())
            .map(|active| active.target)
    })
}

pub(crate) fn is_playing(ui: Ui) -> bool {
    ui.external_playback.with_untracked(|state| {
        state
            .as_ref()
            .is_some_and(|active| active.session.is_open())
    })
}

pub(crate) fn returned(ui: Ui) {
    stop(ui);
    host::external_player_returned();
}

pub(crate) fn stop(ui: Ui) {
    cancel_pending(ui);
    if let Some(active) = ui.external_playback.get_untracked() {
        active.session.close();
        crate::runtime_owner::runtime().parental_end_transfer(&active.target);
    }
}

fn clear_pending_target(ui: Ui, ticket: Option<u64>) {
    let pending = nsh_core::prelude::untracked(|| ui.external_handoff_target.try_get());
    if let Some((pending, target)) = pending.flatten()
        && ticket.is_none_or(|ticket| ticket == pending)
    {
        ui.external_handoff_target.set(None);
        let active = nsh_core::prelude::untracked(|| ui.external_playback.try_get());
        if !active
            .flatten()
            .is_some_and(|active| active.target == target && active.session.is_open())
        {
            crate::runtime_owner::runtime().parental_end_transfer(&target);
        }
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
            .external_handoff_target
            .set(Some((self.ticket, target)));
    }

    fn return_to_internal(&self, target: &Target) {
        if pending_target(self.ui).as_ref() == Some(target) {
            // The existing per-use grant follows the same content back to
            // the native player. Ui::play still checks its ordinary gates.
            self.ui.external_handoff_target.set(None);
        }
    }
}

impl Drop for PendingHandoff {
    fn drop(&mut self) {
        crate::runtime_owner::with_runtime(self.owner.clone(), || {
            clear_pending_target(self.ui, Some(self.ticket))
        });
    }
}

struct PreviousPlayback {
    key: String,
    authority: ResolutionAuthority,
    archive: Option<crate::player::PendingArchive>,
    mode: ViewMode,
}

impl PreviousPlayback {
    fn capture(ui: Ui) -> Option<Self> {
        let channel = ui.playing.get_untracked()?;
        let schedule = crate::runtime_owner::schedule();
        let (epg, local) = iptv_guide::combined::epg_channel(&*schedule, channel)?;
        let archive = match ui.playback.get_untracked() {
            Playback::Live => None,
            Playback::CatchUp { start, offset } => Some(crate::player::PendingArchive::capture(
                &*schedule, channel, start, offset,
            )?),
        };
        Some(Self {
            key: schedule.channel_key(channel)?,
            authority: epg.resolution_authority(local)?,
            archive,
            mode: ui.mode.get_untracked(),
        })
    }

    fn remap(&self, schedule: &dyn Schedule) -> Option<(usize, Playback)> {
        let channel = schedule.find_channel(&self.key)?;
        let (epg, local) = iptv_guide::combined::epg_channel(schedule, channel)?;
        if epg.resolution_authority(local).as_ref() != Some(&self.authority) {
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
            || ui.playing.get_untracked().is_some()
            || crate::on_demand::active(ui)
            || ui.external_playback.get_untracked().is_some()
        {
            return;
        }
        let Some((channel, playback)) = self.remap(&*crate::runtime_owner::schedule()) else {
            return;
        };
        if !crate::player::allow_guide_channel(channel) {
            return;
        }
        let Some(target) = crate::parental_view::live_target(ui, channel) else {
            return;
        };
        if !pending.owner.parental_allowed(&target) {
            return;
        }
        pending.return_to_internal(&target);
        ui.play(channel, playback);
        if ui.playing.get_untracked() == Some(channel) {
            ui.mode.apply(crate::mode::Action::Restore(self.mode));
        } else {
            pending.owner.parental_end_transfer(&target);
        }
    }
}

fn group(ui: Ui) -> Option<(ListId, usize)> {
    let schedule = crate::runtime_owner::schedule();
    let entry = ui.list.with_untracked(|list| list.entry());
    let Entry::Group(group) = Entry::at(entry) else {
        return None;
    };
    schedule
        .group_preferences(group)
        .external_player
        .then(|| (ListId::of(entry, &*schedule), group))
}

pub fn cancel_pending(ui: Ui) {
    ui.external_request
        .update(|generation| *generation = generation.wrapping_add(1));
    clear_pending_target(ui, None);
    if let Some(active) = ui.external_playback.get_untracked()
        && active.session.is_retired()
    {
        clear_retired(ui, &active);
    }
}

pub fn maybe_open(ui: Ui, channel: usize, archive: Option<Program>) -> bool {
    if !host::EXTERNAL_PLAYER || ui.player_surface != 0 {
        return false;
    }
    let Some((list, group_index)) = group(ui) else {
        return false;
    };
    let schedule = crate::runtime_owner::schedule();
    if !schedule.group_channels(group_index).contains(&channel) {
        return false;
    }
    // The player chosen for this group may have been removed since: the
    // caller plays the channel here instead, and the viewer may turn the
    // group's preference off.
    if !host::external_player_available() {
        crate::menu_view::offer_external_player_reset(ui, list);
        crate::settings_view::notify(tr(
            "No external player is installed, so this channel plays here.",
        ));
        return false;
    }
    let owner = crate::runtime_owner::runtime();
    let Some(target) = crate::parental_view::live_target(ui, channel) else {
        return false;
    };
    let Some((epg, local)) = iptv_guide::combined::epg_channel(&*schedule, channel) else {
        return false;
    };
    let Some(authority) = epg.resolution_authority(local) else {
        return false;
    };
    let Some((_, group_id)) =
        crate::data::combined::owner(&owner, &schedule.group_key(group_index))
    else {
        return false;
    };
    let Some(channel_key) = schedule.channel_key(channel) else {
        return false;
    };
    if !crate::player::allow_guide_channel(channel) {
        return true;
    }
    let at = archive.map(|p| crate::guide::unix_of(&*schedule, p.start));
    let clock = crate::runtime::playback::clock(crate::clock::wall_unix(), at);
    let epoch = schedule.epoch_unix();
    let epg_offset = epg.epg_offset_minutes(local);
    let expected = authority.clone();
    let resolve = move || {
        let schedule = crate::runtime_owner::schedule();
        let channel = schedule.find_channel(&channel_key)?;
        let (epg, local) = iptv_guide::combined::epg_channel(&*schedule, channel)?;
        if epg.resolution_authority(local).as_ref() != Some(&authority)
            || (archive.is_some() && epg.epg_offset_minutes(local) != epg_offset)
        {
            return None;
        }
        let archive = archive.map(|p| {
            let offset = epg.minute(epoch);
            Program {
                start: p.start + offset,
                end: p.end + offset,
            }
        });
        Some((epg.resolve(local, archive, clock), epg.shared_playback()))
    };
    cancel_pending(ui);
    let ticket = ui.external_request.get_untracked();
    crate::browse_playback::hold(ui);
    let current = move || {
        ui.external_request.get_untracked() == ticket
            && group(ui).is_some_and(|(now, _)| now == list)
    };
    crate::parental_view::request(ui, target.clone(), move || {
        if !current() {
            return;
        }
        let Some((resolution, context)) = resolve() else {
            unavailable();
            return;
        };
        let config = crate::settings::network_config();
        let owner = crate::runtime_owner::runtime();
        crate::tasks::spawn_in_app(async move {
            let source = if let Resolution::StalkerLink(link) = resolution {
                let StalkerLink {
                    session: mut portal,
                    cmd,
                    channel_headers,
                } = *link;
                let client = owner.link_client(config);
                let cancel = iptv_import::CancelToken::new();
                let result = owner
                    .spawn_network(async move {
                        let net = client.await.ok()?;
                        let url =
                            iptv_import::stalker_link(&net, &mut portal, &cmd, false, &cancel)
                                .await
                                .ok()?;
                        Some((StalkerLink::source(&portal, url, &channel_headers), portal))
                    })
                    .await;
                result.ok().flatten().map(|(source, portal)| {
                    if current()
                        && let Some(context) = context
                    {
                        context.update_stalker_session(portal);
                    }
                    source
                })
            } else {
                resolution.ready()
            };
            if !current() {
                return;
            }
            let Some(mut source) = source else {
                unavailable();
                return;
            };
            crate::settings::add_user_agent(&mut source.headers, crate::settings::user_agent());
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
            if let Some(previous) = ui.external_playback.get_untracked() {
                previous.session.close_and_wait().await;
                clear_retired(ui, &previous);
                if !current() {
                    return;
                }
                if !handoff.is_open() {
                    unavailable();
                    return;
                }
            }
            let Some(lease) = owner.reserve_external_viewing(iptv_library::SourceId(target.source))
            else {
                crate::settings_view::notify(tr(
                    "No provider connection available while recording; stop a recording or check Streams at once",
                ));
                return;
            };
            let previous = PreviousPlayback::capture(ui);
            crate::interruption::viewer_acted(ui);
            crate::player_keys::stopped_for_external(ui, &target);
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
                let active = Active {
                    ticket,
                    target,
                    authority: expected,
                    group: group_id,
                    session,
                };
                ui.external_playback.set(Some(active.clone()));
                crate::tasks::spawn_in_app(async move {
                    active.session.wait_retired().await;
                    clear_retired(ui, &active);
                });
            } else {
                unavailable();
                if let Some(previous) = previous {
                    previous.restore(&pending, current).await;
                }
            }
        });
    });
    true
}

fn unavailable() {
    crate::settings_view::notify(tr("Could not open this channel in an external player."));
}

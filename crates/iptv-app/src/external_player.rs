//! Handoff for an explicit channel choice in a group with external playback.
use crate::{Ui, host, i18n::tr};
use iptv_core::guide::Program;
use iptv_core::parental::Target;
use iptv_guide::playback::{Resolution, StalkerLink};
use iptv_presentation::lists::{Entry, ListId};
use std::rc::Rc;

#[derive(Clone)]
pub(crate) struct Active {
    ticket: u64,
    target: Target,
    authority: iptv_guide::playback::ResolutionAuthority,
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
    if !crate::runtime().parental_allowed(&active.target) {
        active.session.close();
        return;
    }
    let owner = crate::runtime();
    crate::data::spawn_in_app(async move {
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
            crate::runtime().parental_end_transfer(&active.target);
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

pub(crate) fn returned(ui: Ui) {
    cancel_pending(ui);
    host::external_player_returned();
    if let Some(active) = ui.external_playback.get_untracked() {
        active.session.close();
        crate::runtime().parental_end_transfer(&active.target);
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
            crate::runtime().parental_end_transfer(&target);
        }
    }
}

struct PendingHandoff {
    ui: Ui,
    ticket: u64,
    owner: Rc<crate::runtime::Runtime>,
}

impl PendingHandoff {
    fn new(ui: Ui, ticket: u64, target: Target, owner: Rc<crate::runtime::Runtime>) -> Self {
        ui.external_handoff_target.set(Some((ticket, target)));
        Self { ui, ticket, owner }
    }
}

impl Drop for PendingHandoff {
    fn drop(&mut self) {
        crate::with_runtime(self.owner.clone(), || {
            clear_pending_target(self.ui, Some(self.ticket))
        });
    }
}

fn group(ui: Ui) -> Option<(ListId, usize)> {
    let schedule = crate::schedule();
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
}

pub fn maybe_open(ui: Ui, channel: usize, archive: Option<Program>) -> bool {
    if !host::EXTERNAL_PLAYER || ui.player_surface != 0 {
        return false;
    }
    let Some((list, group_index)) = group(ui) else {
        return false;
    };
    let schedule = crate::schedule();
    if !schedule.group_channels(group_index).contains(&channel) {
        return false;
    }
    let owner = crate::runtime();
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
    let clock = crate::runtime::playback::clock(crate::time::wall_unix(), at);
    let epoch = schedule.epoch_unix();
    let epg_offset = epg.epg_offset_minutes(local);
    let expected = authority.clone();
    let resolve = move || {
        let schedule = crate::schedule();
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
        let owner = crate::runtime();
        crate::data::spawn_in_app(async move {
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
            let prepared = host::prepare_external_player(source).await;
            if !current() {
                return;
            }
            let Some(prepared) = prepared else {
                unavailable();
                return;
            };
            let _pending = PendingHandoff::new(ui, ticket, target.clone(), owner.clone());
            let Some(mut handoff) = host::open_external_player(prepared) else {
                unavailable();
                return;
            };
            if !handoff.connected().await || !current() {
                return;
            }
            let verified = owner
                .external_playback_allowed(expected.clone(), group_id.clone())
                .await;
            if !current() || !handoff.is_open() {
                return;
            }
            if !matches!(verified, Ok(true))
                || !owner.parental_allowed(&target)
                || resolve().is_none()
            {
                unavailable();
                return;
            }
            if let Some(previous) = ui.external_playback.get_untracked() {
                previous.session.close_and_wait().await;
                clear_retired(ui, &previous);
                if !current() || !handoff.is_open() {
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
            crate::interruption::viewer_acted(ui);
            crate::player_keys::stopped_for_external(ui, &target);
            owner.retire_viewing_inputs(0).await;
            if !current() || !handoff.is_open() {
                return;
            }
            let verified = owner
                .external_playback_allowed(expected.clone(), group_id.clone())
                .await;
            if !current() || !handoff.is_open() {
                return;
            }
            if !matches!(verified, Ok(true))
                || !owner.parental_allowed(&target)
                || resolve().is_none()
            {
                unavailable();
                return;
            }
            if let Some(session) = handoff.allow(lease) {
                ui.external_playback.set(Some(Active {
                    ticket,
                    target,
                    authority: expected,
                    group: group_id,
                    session,
                }));
            } else {
                unavailable();
            }
        });
    });
    true
}

fn unavailable() {
    crate::settings_view::notify(tr("Could not open this channel in an external player."));
}

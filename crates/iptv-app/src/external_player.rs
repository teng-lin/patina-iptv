//! Handoff for an explicit channel choice in a group with external playback.
use crate::{Ui, host, i18n::tr};
use iptv_core::guide::Program;
use iptv_guide::playback::{Resolution, StalkerLink};
use iptv_presentation::lists::{Entry, ListId};

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
            let Some(mut handoff) = prepared.and_then(host::open_external_player) else {
                unavailable();
                return;
            };
            if !handoff.connected().await || !current() {
                return;
            }
            let verified = owner.external_playback_allowed(expected, group_id).await;
            if !current() {
                return;
            }
            if !matches!(verified, Ok(true))
                || !owner.parental_allowed(&target)
                || resolve().is_none()
            {
                unavailable();
                return;
            }
            let Some(lease) = owner.reserve_external_viewing(iptv_library::SourceId(target.source))
            else {
                crate::settings_view::notify(tr(
                    "No provider connection available while recording; stop a recording or check Streams at once",
                ));
                return;
            };
            crate::interruption::viewer_acted(ui);
            crate::player_keys::stopped(ui);
            owner.retire_viewing_inputs(0).await;
            if current() && owner.parental_allowed(&target) {
                handoff.allow(lease);
            }
        });
    });
    true
}

fn unavailable() {
    crate::settings_view::notify(tr("Could not open this channel in an external player."));
}

//! Intent handling for the Cast picker and remote.
use crate::{
    Playback, Ui, chooser,
    i18n::tr,
    runtime_owner::{runtime, schedule},
    settings_view::notify,
};
use iptv_core::{
    capabilities::FormFactor,
    cast::{Command, Error, Receiver, RemoteState},
};
use nsh_core::prelude::set_timeout;
use std::time::Duration;

pub(super) fn available(ui: Ui) -> bool {
    runtime().cast_available()
        && crate::device::host().form_factor != FormFactor::Tv
        && ui.player_surface == 0
        && !crate::multiview::is_open(ui)
        && !crate::pip::in_window(ui)
        && (ui.cast_remote.get()
            || (ui.playing.get().is_some() && ui.playback.get() == Playback::Live))
}

pub(super) fn local_blocked(ui: Ui) -> bool {
    if ui.cast_remote.get_untracked() || ui.cast_busy.get_untracked() {
        notify(tr("Stop casting before starting local playback."));
        true
    } else {
        false
    }
}

pub(super) fn open(ui: Ui) {
    if ui.cast_busy.get_untracked() {
        return;
    }
    if ui.cast_remote.get_untracked() {
        chooser::open(ui, chooser::Kind::CastRemote);
        return;
    }
    ui.cast_busy.set(true);
    let channel = ui.player.main().key.get_untracked();
    crate::tasks::spawn_in_app(async move {
        let result = runtime().cast_receivers().await;
        ui.cast_busy.set(false);
        if channel != ui.player.main().key.get_untracked() || !available(ui) {
            return;
        }
        match result {
            Ok(receivers) if !receivers.is_empty() => chooser::show_receivers(ui, receivers),
            Ok(_) => notify(tr("No Chromecast found on this network.")),
            Err(e) => report(e),
        }
    });
}
pub(super) fn select(ui: Ui, receiver: Receiver) {
    if let Some(channel) = ui.playing.get_untracked() {
        start(ui, channel, receiver);
    }
}
pub(super) fn change(ui: Ui, channel: usize) -> bool {
    if let Some((receiver, _)) = runtime().cast_target() {
        start(ui, channel, receiver);
        true
    } else {
        false
    }
}
fn start(ui: Ui, channel: usize, receiver: Receiver) {
    if ui.cast_busy.get_untracked() {
        return;
    }
    let guide = schedule();
    let Some(key) = guide.channel_key(channel) else {
        return;
    };
    let Some((epg, local)) = iptv_guide::combined::epg_channel(&*guide, channel) else {
        return;
    };
    let Some(authority) = epg.resolution_authority(local) else {
        return;
    };
    let resolution = epg.resolve(
        local,
        None,
        crate::runtime::playback::clock(crate::clock::wall_unix(), None),
    );
    let context = epg.shared_playback();
    let Some(target) = crate::parental_view::live_target(ui, channel) else {
        return;
    };
    let owner = runtime();
    let previous = owner
        .cast_target()
        .map(|(_, key)| key)
        .or_else(|| ui.player.main().key.get_untracked());
    let old_paused = ui.paused.get_untracked();
    let ticket = owner.next_cast_intent();
    let current = move || runtime().cast_intent_current(ticket);
    ui.cast_busy.set(true);
    crate::tasks::spawn_in_app(async move {
        let mut retired = false;
        let mut transferred = false;
        let result = async {
            let check = || async {
                let grant = owner
                    .authorize_cast_playback(authority.clone())
                    .await
                    .ok()
                    .flatten()
                    .ok_or(Error::Denied)?;
                if !current() || !grant.is_current() {
                    return Err(Error::Denied);
                }
                Ok(grant)
            };
            check().await?;
            let mut source = match resolution {
                iptv_guide::playback::Resolution::Play(source) => source,
                iptv_guide::playback::Resolution::StalkerLink(link) => {
                    let iptv_guide::playback::StalkerLink {
                        session: mut portal,
                        cmd,
                        channel_headers,
                    } = *link;
                    let client = owner.link_client(crate::settings::network_config());
                    let (source, portal) = owner
                        .spawn_network(async move {
                            let net = client.await.ok()?;
                            let url = iptv_import::stalker_link(
                                &net,
                                &mut portal,
                                &cmd,
                                false,
                                &iptv_import::CancelToken::new(),
                            )
                            .await
                            .ok()?;
                            Some((
                                iptv_guide::playback::StalkerLink::source(
                                    &portal,
                                    url,
                                    &channel_headers,
                                ),
                                portal,
                            ))
                        })
                        .await
                        .ok()
                        .flatten()
                        .ok_or(Error::Unavailable)?;
                    check().await?;
                    if let Some(context) = context {
                        context.update_stalker_session(portal);
                    }
                    source
                }
                _ => return Err(Error::Unsupported),
            };
            crate::settings::add_user_agent(&mut source.headers, crate::settings::user_agent());
            if owner.cast_target().is_some() {
                owner.cast_command(Command::Stop).await?;
                retired = true;
                ui.cast_remote.set(false);
            }
            let prepared = owner.prepare_cast(receiver.clone(), source).await?;
            let grant = check().await?;
            let claim = owner
                .reserve_cast_viewing(iptv_library::SourceId(authority.source()))
                .ok_or(Error::Busy)?;
            ui.paused.set(true);
            let prepared = owner.load_cast(ticket, prepared, grant).await?;
            check().await?;
            crate::player::stop_for_handoff(ui, &target);
            transferred = true;
            retired = true;
            owner.retire_viewing_inputs(0).await;
            let grant = check().await?;
            owner
                .admit_cast(
                    ticket,
                    prepared,
                    iptv_runtime::cast::Destination {
                        receiver,
                        key: key.clone(),
                        authority: authority.clone(),
                    },
                    claim,
                    grant,
                )
                .await?;
            if !current() {
                if owner.cast_command(Command::Stop).await.is_err() {
                    monitor(ui, ticket);
                }
                // Active owns the transferred grant until clear_cast retires it.
                transferred = false;
                return Err(Error::Denied);
            }
            ui.player.main().key.set(Some(key.clone()));
            ui.playing.set(schedule().find_channel(&key));
            ui.cast_remote.set(true);
            ui.paused.set(false);
            monitor(ui, ticket);
            Ok(())
        }
        .await;
        ui.cast_busy.set(false);
        if let Err(e) = result {
            report(e);

            if current() {
                ui.paused.set(old_paused);
                if retired
                    && let Some(ch) = previous
                        .as_deref()
                        .and_then(|key| schedule().find_channel(key))
                {
                    ui.playing.set(None);
                    ui.play(ch, Playback::Live);
                    if ui.playing.get_untracked() == Some(ch) {
                        ui.paused.set(old_paused);
                    }
                    if transferred
                        && ui.playing.get_untracked() == Some(ch)
                        && crate::parental_view::live_target(ui, ch).as_ref() == Some(&target)
                    {
                        transferred = false;
                    }
                }
            }
            if transferred {
                owner.parental_end_transfer(&target);
            }
        }
        finish_pending_stop(ui);
    });
}
fn monitor(ui: Ui, ticket: u64) {
    let _timer = set_timeout(Duration::from_secs(3), move || {
        if runtime().active_cast_ticket() != Some(ticket) {
            return;
        }
        crate::tasks::spawn_in_app(async move {
            match runtime().poll_cast(ticket).await {
                Ok(RemoteState::Ended)
                    if runtime().cast_target().is_none() && ui.cast_remote.get_untracked() =>
                {
                    ui.cast_remote.set(false);
                    ui.playing.set(None);
                    if !ui.cast_busy.get_untracked() {
                        chooser::open(ui, chooser::Kind::CastEnded);
                    }
                }
                Ok(state) => {
                    if runtime().active_cast_ticket() == Some(ticket) {
                        if matches!(state, RemoteState::Playing | RemoteState::Paused) {
                            ui.paused.set(state == RemoteState::Paused);
                        }
                        monitor(ui, ticket);
                    }
                }
                Err(_) if runtime().active_cast_ticket() == Some(ticket) => monitor(ui, ticket),
                Err(_) => {}
            }
        });
    });
}
pub(super) fn remote(ui: Ui, index: usize) {
    let command = match index {
        0 => Command::Pause,
        1 => Command::Play,
        2 | 3 => Command::Stop,
        _ => return,
    };
    if ui.cast_busy.get_untracked() {
        if index == 3 {
            ui.cast_stop_pending.set(true);
        }
        return;
    }
    let Some((_, key)) = runtime().cast_target() else {
        return;
    };
    ui.cast_busy.set(true);
    crate::tasks::spawn_in_app(async move {
        let result = runtime().cast_command(command).await;
        ui.cast_busy.set(false);
        match result {
            Ok(()) if command == Command::Stop => {
                ui.cast_remote.set(false);
                ui.playing.set(None);
                if index == 2
                    && let Some(ch) = schedule().find_channel(&key)
                {
                    ui.play(ch, Playback::Live);
                }
            }
            Ok(()) => ui.paused.set(command == Command::Pause),
            Err(e) => report(e),
        }
        finish_pending_stop(ui);
    });
}
fn finish_pending_stop(ui: Ui) {
    if ui.cast_stop_pending.get_untracked() {
        ui.cast_stop_pending.set(false);
        if ui.cast_remote.get_untracked() {
            remote(ui, 3);
        }
    }
}
pub(crate) fn volume(ui: Ui, command: Command) -> bool {
    if !ui.cast_remote.get_untracked() {
        return false;
    }
    crate::tasks::spawn_in_app(async move {
        if let Err(error) = runtime().cast_command(command).await {
            report(error);
        }
    });
    true
}
fn report(error: Error) {
    notify(tr(match error {
        Error::Unavailable => "Could not connect to the Chromecast or open the relay port.",
        Error::Refused => "The Chromecast refused this stream.",
        Error::Unknown => "Could not confirm playback on the Chromecast.",
        Error::NoRequest => {
            "The TV did not request the stream. Check the firewall and Wi-Fi client isolation."
        }
        Error::Busy => "No provider connection is available.",
        Error::Denied => "Playback is no longer authorized.",
        Error::Unsupported => "Casting requires a live HLS channel.",
    }));
}

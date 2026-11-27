//! Intent handling for the Cast picker and remote.
use crate::{
    Playback, Ui, chooser,
    i18n::{tr, trf},
    runtime_owner::{runtime, schedule},
    settings_view::notify,
};
use iptv_core::{
    capabilities::FormFactor,
    cast::{Command, Error, Receiver, RemoteState, Target},
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
    discover(ui);
}
pub(super) fn discover(ui: Ui) {
    if ui.cast_discovering.get_untracked() {
        return;
    }
    chooser::show_receivers(ui, Vec::new());
    if !runtime().chromecast_available() {
        return;
    }
    ui.cast_discovering.set(true);
    let channel = ui.player.main().key.get_untracked();
    crate::tasks::spawn_in_app(async move {
        let result = runtime().cast_receivers().await;
        ui.cast_discovering.set(false);
        let still_open = ui.chooser.with_untracked(|open| {
            open.as_ref()
                .is_some_and(|open| open.kind == chooser::Kind::Cast)
        });
        if !still_open || channel != ui.player.main().key.get_untracked() || !available(ui) {
            return;
        }
        // Keep current and local destinations usable when discovery fails.
        if let Ok(receivers) = result {
            chooser::show_receivers(ui, receivers);
        }
    });
}
pub(super) fn destination_selected(value: &str) -> bool {
    match runtime().cast_target() {
        None => value == "local",
        Some((Target::Chromecast(receiver), _)) => value == receiver.address.to_string(),
        Some((Target::AirPlay, _)) => false, // Selection is shown by Apple's named-device picker.
    }
}
pub(super) fn play_here(ui: Ui) {
    if ui.cast_busy.get_untracked() {
        let owner = runtime();
        let ticket = if ui.cast_remote.get_untracked() {
            // Let an outstanding command finish on the current receiver.
            owner.cast_intent()
        } else {
            // The native port may not have begun preparing a generation yet.
            // Invalidate the runtime intent as well as any active native wait.
            let ticket = owner.next_cast_intent();
            owner.cancel_pending_native_cast();
            ticket
        };
        ui.cast_local_pending.set(Some(ticket));
    } else if ui.cast_remote.get_untracked() {
        control(ui, Command::Stop, true);
    }
}
pub(super) fn select(ui: Ui, receiver: Receiver) {
    if runtime()
        .cast_target()
        .is_some_and(|(target, _)| matches!(target, Target::Chromecast(current) if current.address == receiver.address))
    {
        play_here(ui);
        return;
    }
    if let Some(channel) = ui.playing.get_untracked() {
        start(ui, channel, Target::Chromecast(receiver));
    }
}
pub(super) fn select_airplay(ui: Ui) {
    if runtime().native_cast_available()
        && !ui.cast_busy.get_untracked()
        && !runtime()
            .cast_target()
            .is_some_and(|(target, _)| target == Target::AirPlay)
        && let Some(channel) = ui.playing.get_untracked()
    {
        start(ui, channel, Target::AirPlay);
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
fn start(ui: Ui, channel: usize, receiver: Target) {
    if ui.cast_busy.get_untracked() {
        return;
    }
    let airplay = receiver == Target::AirPlay;
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
    if airplay && runtime().native_cast_view().is_none() {
        chooser::open(ui, chooser::Kind::AirPlay);
    }
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
                if airplay
                    && owner
                        .cast_target()
                        .is_some_and(|(target, _)| target == Target::AirPlay)
                {
                    owner.replace_native_cast().await?;
                } else {
                    owner.cast_command(Command::Stop).await?;
                }
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
            let paused = owner
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
            ui.paused.set(paused);
            monitor(ui, ticket);
            Ok(())
        }
        .await;
        if airplay && result.is_err() {
            owner.flush_native_cast().await;
        }
        ui.cast_busy.set(false);
        if let Err(e) = result {
            let returning_local = pending_local_current(ui);
            if !returning_local {
                report(e);
            }

            if retired && pending_stop_current(ui) {
                // The old receiver is already stopped. Preserve the viewer's Stop
                // even when the switch failed after releasing its local surface.
                ui.playing.set(None);
            } else if current() || returning_local {
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
        if ui.chooser.with_untracked(|open| {
            open.as_ref().is_some_and(|open| {
                matches!(open.kind, chooser::Kind::AirPlay | chooser::Kind::Cast)
            })
        }) {
            chooser::close(ui);
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
pub(crate) fn set_paused(ui: Ui, paused: bool) {
    if ui.paused.get_untracked() != paused {
        control(
            ui,
            if paused {
                Command::Pause
            } else {
                Command::Play
            },
            false,
        );
    }
}
pub(crate) fn stop(ui: Ui) {
    control(ui, Command::Stop, false);
}
fn control(ui: Ui, command: Command, resume_local: bool) {
    if ui.cast_busy.get_untracked() {
        if command == Command::Stop && !resume_local {
            ui.cast_stop_pending.set(Some(runtime().cast_intent()));
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
                if resume_local
                    && !pending_stop_current(ui)
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
fn pending_stop_current(ui: Ui) -> bool {
    ui.cast_stop_pending
        .get_untracked()
        .is_some_and(|ticket| runtime().cast_intent_current(ticket))
}
fn pending_local_current(ui: Ui) -> bool {
    ui.cast_local_pending
        .get_untracked()
        .is_some_and(|ticket| runtime().cast_intent_current(ticket))
}
fn finish_pending_stop(ui: Ui) {
    let stop_requested = pending_stop_current(ui);
    ui.cast_stop_pending.set(None);
    let local = pending_local_current(ui);
    ui.cast_local_pending.set(None);
    if ui.cast_remote.get_untracked() {
        if stop_requested {
            stop(ui);
        } else if local {
            chooser::close(ui);
            control(ui, Command::Stop, true);
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
    log::debug!("cast operation result: {error:?}");
    match error {
        Error::Cancelled => {}
        Error::NoRequest { relay_port } => notify(&trf(
            "The TV did not request the stream. Allow TCP port {port} from the TV in your firewall and check Wi-Fi client isolation.",
            &[("port", relay_port.into())],
        )),
        Error::Unavailable | Error::Refused | Error::Unknown => {
            notify(tr("Could not start playback on the selected device."))
        }
        Error::Busy => notify(tr("No provider connection is available.")),
        Error::Denied => notify(tr("Playback is no longer authorized.")),
        Error::Unsupported => notify(tr("This channel cannot be cast.")),
    }
}

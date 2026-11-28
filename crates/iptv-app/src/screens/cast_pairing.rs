//! Receiver approval belongs to the window that chose the destination.
use crate::{
    Ui,
    kit::i18n::{tr, trf},
    kit::overlay::Overlay,
    kit::scale::Scale,
    kit::ui_kit,
    kit::ui_kit::entry::{frame, sheet_buttons, sheet_field},
    wiring::runtime_owner::runtime,
};
use iptv_core::cast::{Error, Receiver};
use iptv_presentation::cast::{Effect, FirewallHelp, Prompt, State};
use iptv_runtime::cast::{PairingPrompt, pairing_prompt};
use nsh_core::prelude::*;

pub(crate) fn prompt(cast: crate::screens::casting::CastUi, ticket: u64) -> PairingPrompt {
    let (prompt, request) = pairing_prompt();
    crate::wiring::tasks::spawn_in_app(async move {
        if let Ok(request) = request.await
            && runtime().cast_intent_current(ticket)
            && cast.busy.try_get() == Some(true)
        {
            cast.pairing.replace(Some(request));
            cast.state.update(|state| state.open_pin(ticket));
        }
    });
    prompt
}

pub(crate) fn offer_forget(
    cast: crate::screens::casting::CastUi,
    ticket: u64,
    receiver: Receiver,
    reason: Error,
) {
    cast.pairing.replace(None);
    cast.state
        .update(|state| state.offer_forget(ticket, receiver, reason));
}

pub(crate) fn show_linux_airplay_setup(cast: crate::screens::casting::CastUi) {
    let tcp_port = configured_port("PATINA_CAST_RELAY_PORT", 49152);
    let udp_port = configured_port("PATINA_AIRPLAY_TIMING_PORT", 49153);
    let subnet = crate::host::local_ipv4_subnet().unwrap_or_else(|| "192.168.1.0/24".into());
    cast.pairing.replace(None);
    cast.state.update(|state| {
        state.open_firewall_help(FirewallHelp {
            peer_ip: None,
            tcp_port: Some(tcp_port),
            udp_port: Some(udp_port),
            subnet: Some(subnet),
            copy_selected: true,
        });
    });
}

pub(crate) fn show_firewall_blocked(
    cast: crate::screens::casting::CastUi,
    peer_ip: String,
    tcp_port: Option<u16>,
    udp_port: Option<u16>,
) {
    cast.pairing.replace(None);
    cast.state.update(|state| {
        state.open_firewall_help(FirewallHelp {
            peer_ip: Some(peer_ip),
            tcp_port,
            udp_port,
            subnet: None,
            copy_selected: true,
        });
    });
}

fn configured_port(env_var: &str, default: u16) -> u16 {
    std::env::var(env_var)
        .ok()
        .and_then(|value| value.parse::<std::num::NonZeroU16>().ok())
        .map_or(default, std::num::NonZeroU16::get)
}

fn firewall_commands(help: &FirewallHelp) -> String {
    firewall_commands_for(help, crate::host::detect_linux_firewall())
}

fn firewall_commands_for(help: &FirewallHelp, firewall: crate::host::LinuxFirewall) -> String {
    let source = help
        .peer_ip
        .as_deref()
        .or(help.subnet.as_deref())
        .unwrap_or("192.168.1.0/24");
    let family = if source.contains(':') { "ipv6" } else { "ipv4" };
    let mut lines = Vec::new();
    for (port, proto) in [(help.tcp_port, "tcp"), (help.udp_port, "udp")] {
        let Some(port) = port else { continue };
        match firewall {
            crate::host::LinuxFirewall::Ufw => lines.push(format!(
                "sudo ufw allow from {source} to any port {port} proto {proto}"
            )),
            crate::host::LinuxFirewall::Firewalld => lines.push(format!(
                "sudo firewall-cmd --add-rich-rule='rule family=\"{family}\" source address=\"{source}\" port port=\"{port}\" protocol=\"{proto}\" accept'"
            )),
        }
    }
    lines.join("\n")
}

fn copy_firewall_commands(cast: crate::screens::casting::CastUi) {
    let commands = cast
        .state
        .with_untracked(|state| state.firewall_help().map(firewall_commands));
    if let Some(commands) = commands
        && !commands.is_empty()
    {
        set_clipboard(commands);
        crate::kit::toast::notify(tr("Copied firewall command."));
    }
}

pub(crate) fn is_open(cast: crate::screens::casting::CastUi) -> bool {
    cast.prompt.with(Option::is_some)
}

pub(crate) fn close_for(cast: crate::screens::casting::CastUi, ticket: u64) {
    if untracked(|| cast.prompt.try_get()).is_some()
        && cast.state.with_untracked(State::pairing_ticket) == Some(ticket)
    {
        close(cast);
    }
}

pub(crate) fn close(cast: crate::screens::casting::CastUi) {
    cast.pairing.replace(None);
    cast.state.update(State::close_pairing);
    dismiss_keyboard();
}

fn submit(cast: crate::screens::casting::CastUi) {
    if cast.prompt.get_untracked() != Some(Prompt::Pin) {
        return;
    }
    let mut effect = None;
    cast.state.update(|state| effect = state.submit_pin());
    let Some(Effect::SubmitPin { ticket, pin }) = effect else {
        crate::kit::toast::notify(tr("Enter the four digits shown on the receiver."));
        return;
    };
    if let Some(request) = cast.pairing.replace(None) {
        close(cast);
        if runtime().cast_intent_current(ticket) {
            let _ = request.submit(pin);
        }
    }
}

fn forget(ui: Ui) {
    if ui.cast.busy.get_untracked() {
        return;
    }
    let Some((ticket, receiver)) = ui.cast.state.with_untracked(State::forget_receiver) else {
        return;
    };
    close(ui.cast);
    if !runtime().cast_intent_current(ticket) {
        return;
    }
    ui.cast.state.update(|state| state.busy = true);
    crate::wiring::tasks::spawn_in_app(async move {
        let result = runtime().forget_cast_pairing(receiver).await;
        if ui.cast.busy.try_get().is_some() {
            ui.cast.state.update(|state| state.busy = false);
            if result.is_err() {
                crate::kit::toast::notify(tr("Could not forget the AirPlay pairing."));
            }
            crate::screens::casting::finish_pending_stop(ui);
        }
    });
}

pub(crate) fn key(ui: Ui, action: iptv_nav::Action) {
    use iptv_nav::Action;
    if let Some(Prompt::Forget { selected, .. }) = ui.cast.prompt.get_untracked() {
        match action {
            Action::MoveLeft | Action::MoveRight => ui
                .cast
                .state
                .update(|state| state.select_forget(action == Action::MoveRight)),
            Action::Select if selected => forget(ui),
            Action::Select | Action::Back | Action::ShowMenu => close(ui.cast),
            _ => {}
        }
        return;
    }
    if let Some(Prompt::Firewall { copy_selected }) = ui.cast.prompt.get_untracked() {
        match action {
            Action::MoveLeft | Action::MoveRight => ui
                .cast
                .state
                .update(|state| state.select_firewall_copy(action == Action::MoveLeft)),
            Action::Select if copy_selected => copy_firewall_commands(ui.cast),
            Action::Select | Action::Back | Action::ShowMenu => close(ui.cast),
            _ => {}
        }
        return;
    }
    match action {
        Action::Back | Action::ShowMenu => close(ui.cast),
        Action::Select => submit(ui.cast),
        _ => {}
    }
}

pub(crate) fn layer(ui: Ui) -> impl IntoElement {
    when(
        move || is_open(ui.cast),
        move || {
            let scale = Scale::new();
            let (outer, card) = frame(ui, scale);
            outer
                .accessibility_elements_hidden(move || {
                    Overlay::CastPairing.accessibility_hidden(ui)
                })
                .child(
                    card.child(
                        when(
                            move || matches!(ui.cast.prompt.get(), Some(Prompt::Forget { .. })),
                            move || forget_card(ui, scale),
                        )
                        .otherwise(move || {
                            when(
                                move || {
                                    matches!(ui.cast.prompt.get(), Some(Prompt::Firewall { .. }))
                                },
                                move || firewall_card(ui, scale),
                            )
                            .otherwise(move || pin_card(ui, scale))
                        }),
                    ),
                )
        },
    )
}

fn firewall_card(ui: Ui, scale: Scale) -> View {
    let wants_copy = move || {
        matches!(
            ui.cast.prompt.get(),
            Some(Prompt::Firewall {
                copy_selected: true
            })
        )
    };
    let help = move || ui.cast.state.with(|state| state.firewall_help().cloned());
    view()
        .gap(move || scale.px(14.0))
        .child(ui_kit::title(scale, move || {
            if help().is_some_and(|h| h.peer_ip.is_some()) {
                tr("Blocked by this computer's firewall")
            } else {
                tr("Allow AirPlay in this computer's firewall")
            }
        }))
        .child(ui_kit::body(scale, move || match help() {
            Some(FirewallHelp {
                peer_ip: Some(ip), ..
            }) => trf(
                "Your TV or AirPlay device ({ip}) could not reach this computer. Run this in a terminal to allow it:",
                &[("ip", ip.as_str().into())],
            ),
            _ => tr(
                "Your TV or AirPlay device connects back to this computer. Run this in a terminal to allow it (or replace the subnet with one device's IP):",
            )
            .to_owned(),
        }))
        .child(
            view()
                .p(move || scale.px(12.0))
                .rounded(px(crate::theme::RADIUS))
                .bg(crate::tint(0x000000, 0x60))
                .child(ui_kit::caption(scale, move || {
                    help().map_or_else(String::new, |h| firewall_commands(&h))
                })),
        )
        .child(
            view()
                .flex_row()
                .gap(move || scale.px(12.0))
                .justify_end()
                .child(ui_kit::button(
                    ui,
                    scale,
                    tr("Copy command"),
                    wants_copy,
                    move || copy_firewall_commands(ui.cast),
                ))
                .child(ui_kit::button(
                    ui,
                    scale,
                    tr("Done"),
                    move || !wants_copy(),
                    move || close(ui.cast),
                )),
        )
}

fn forget_card(ui: Ui, scale: Scale) -> View {
    let wants_forget = move || {
        matches!(
            ui.cast.prompt.get(),
            Some(Prompt::Forget { selected: true, .. })
        )
    };
    view()
        .gap(move || scale.px(14.0))
        .child(ui_kit::title(scale, move || {
            let storage = matches!(
                ui.cast.prompt.get(),
                Some(Prompt::Forget {
                    reason: Error::PairingInvalid,
                    ..
                })
            );
            if storage {
                tr("The saved AirPlay pairing is invalid.")
            } else {
                tr("The AirPlay receiver identity changed.")
            }
        }))
        .child(ui_kit::body(
            scale,
            tr("Forget this pairing to choose the receiver again."),
        ))
        .child(
            view()
                .flex_row()
                .gap(move || scale.px(12.0))
                .justify_end()
                .child(ui_kit::button(
                    ui,
                    scale,
                    tr("Cancel"),
                    move || !wants_forget(),
                    move || close(ui.cast),
                ))
                .child(
                    ui_kit::button(ui, scale, tr("Forget pairing"), wants_forget, move || {
                        forget(ui)
                    })
                    .disabled(move || ui.cast.busy.get()),
                ),
        )
}

fn pin_card(ui: Ui, scale: Scale) -> View {
    view()
        .gap(move || scale.px(14.0))
        .child(ui_kit::title(scale, tr("Pair with AirPlay")))
        .child(ui_kit::body(
            scale,
            tr("Approve pairing on the receiver and enter the four-digit code it shows."),
        ))
        .child(
            sheet_field(ui, scale)
                .test_id("cast-pairing-pin")
                .label(|| tr("AirPlay PIN"))
                .placeholder(tr("AirPlay PIN"))
                .auto_focus(true)
                .masked(true)
                .keyboard_type(KeyboardType::Number)
                .max_length(4)
                .auto_correct(false)
                .on_change(move |text: &str| ui.cast.state.update(|state| state.set_pin(text)))
                .on_submit(move |_: &str| submit(ui.cast)),
        )
        .child(sheet_buttons(
            ui,
            scale,
            |ui| close(ui.cast),
            |ui| submit(ui.cast),
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firewall_commands_format_subnet_and_exact_peer_rules() {
        use crate::host::LinuxFirewall;

        let setup = FirewallHelp {
            peer_ip: None,
            tcp_port: Some(49152),
            udp_port: Some(49153),
            subnet: Some("192.0.2.0/24".into()),
            copy_selected: true,
        };
        assert_eq!(
            firewall_commands_for(&setup, LinuxFirewall::Ufw),
            "sudo ufw allow from 192.0.2.0/24 to any port 49152 proto tcp\n\
             sudo ufw allow from 192.0.2.0/24 to any port 49153 proto udp"
        );
        assert_eq!(
            firewall_commands_for(&setup, LinuxFirewall::Firewalld),
            "sudo firewall-cmd --add-rich-rule='rule family=\"ipv4\" source address=\"192.0.2.0/24\" port port=\"49152\" protocol=\"tcp\" accept'\n\
             sudo firewall-cmd --add-rich-rule='rule family=\"ipv4\" source address=\"192.0.2.0/24\" port port=\"49153\" protocol=\"udp\" accept'"
        );

        let blocked = FirewallHelp {
            peer_ip: Some("192.0.2.10".into()),
            tcp_port: None,
            udp_port: Some(49153),
            subnet: None,
            copy_selected: true,
        };
        assert_eq!(
            firewall_commands_for(&blocked, LinuxFirewall::Ufw),
            "sudo ufw allow from 192.0.2.10 to any port 49153 proto udp"
        );
    }
}

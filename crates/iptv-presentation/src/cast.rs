//! State for the Cast destination picker, remote, and AirPlay pairing prompt.

use iptv_core::cast::{Controls, Error, Receiver};
use zeroize::{Zeroize, Zeroizing};

#[derive(Clone, PartialEq, Eq)]
pub struct State {
    pub busy: bool,
    pub controls: Controls,
    pub picker_open: bool,
    pub discovering: bool,
    pub local_pending: Option<u64>,
    pub stop_pending: Option<u64>,
    pub remote: bool,
    pairing: Option<Pairing>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            busy: false,
            controls: Controls::NONE,
            picker_open: false,
            discovering: false,
            local_pending: None,
            stop_pending: None,
            remote: false,
            pairing: None,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Pairing {
    ticket: u64,
    request: Request,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirewallHelp {
    pub peer_ip: Option<String>,
    pub tcp_port: Option<u16>,
    pub udp_port: Option<u16>,
    pub subnet: Option<String>,
    pub copy_selected: bool,
}

#[derive(Clone, PartialEq, Eq)]
enum Request {
    Pin(Zeroizing<String>),
    Forget {
        receiver: Receiver,
        reason: Error,
        selected: bool,
    },
    Firewall(FirewallHelp),
}

/// A projection suitable for rendering. It contains no PIN or receiver handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prompt {
    Pin,
    Forget { reason: Error, selected: bool },
    Firewall { copy_selected: bool },
}

pub enum Effect {
    SubmitPin { ticket: u64, pin: Zeroizing<String> },
}

impl State {
    pub fn pairing_ticket(&self) -> Option<u64> {
        self.pairing.as_ref().map(|pairing| pairing.ticket)
    }

    pub fn prompt(&self) -> Option<Prompt> {
        self.pairing.as_ref().map(|pairing| match &pairing.request {
            Request::Pin(_) => Prompt::Pin,
            Request::Forget {
                reason, selected, ..
            } => Prompt::Forget {
                reason: *reason,
                selected: *selected,
            },
            Request::Firewall(help) => Prompt::Firewall {
                copy_selected: help.copy_selected,
            },
        })
    }

    pub fn open_firewall_help(&mut self, help: FirewallHelp) {
        self.pairing = Some(Pairing {
            ticket: 0,
            request: Request::Firewall(help),
        });
    }

    pub fn firewall_help(&self) -> Option<&FirewallHelp> {
        let Pairing {
            request: Request::Firewall(help),
            ..
        } = self.pairing.as_ref()?
        else {
            return None;
        };
        Some(help)
    }

    pub fn select_firewall_copy(&mut self, value: bool) {
        if let Some(Pairing {
            request: Request::Firewall(help),
            ..
        }) = &mut self.pairing
        {
            help.copy_selected = value;
        }
    }

    pub fn open_pin(&mut self, ticket: u64) {
        self.pairing = Some(Pairing {
            ticket,
            request: Request::Pin(Zeroizing::new(String::new())),
        });
    }

    pub fn offer_forget(&mut self, ticket: u64, receiver: Receiver, reason: Error) {
        self.pairing = Some(Pairing {
            ticket,
            request: Request::Forget {
                receiver,
                reason,
                selected: false,
            },
        });
    }

    pub fn close_pairing(&mut self) {
        self.pairing = None;
    }

    pub fn set_pin(&mut self, text: &str) {
        if let Some(Pairing {
            request: Request::Pin(pin),
            ..
        }) = &mut self.pairing
        {
            pin.zeroize();
            if text.len() <= 4 && text.bytes().all(|digit| digit.is_ascii_digit()) {
                pin.push_str(text);
            }
        }
    }

    pub fn submit_pin(&mut self) -> Option<Effect> {
        let Pairing {
            ticket,
            request: Request::Pin(pin),
        } = self.pairing.as_mut()?
        else {
            return None;
        };
        (pin.len() == 4).then(|| Effect::SubmitPin {
            ticket: *ticket,
            pin: std::mem::take(pin),
        })
    }

    pub fn select_forget(&mut self, value: bool) {
        if let Some(Pairing {
            request: Request::Forget { selected, .. },
            ..
        }) = &mut self.pairing
        {
            *selected = value;
        }
    }

    pub fn forget_receiver(&self) -> Option<(u64, Receiver)> {
        let Pairing {
            ticket,
            request: Request::Forget { receiver, .. },
        } = self.pairing.as_ref()?
        else {
            return None;
        };
        Some((*ticket, receiver.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_owns_a_single_use_pin_for_its_ticket() {
        let mut state = State::default();
        state.open_pin(13);
        state.set_pin("1x34");
        assert!(state.submit_pin().is_none());
        state.set_pin("1234");
        let Some(Effect::SubmitPin { ticket, pin }) = state.submit_pin() else {
            panic!("a four-digit PIN is accepted");
        };
        assert_eq!(ticket, 13);
        assert_eq!(&**pin, "1234");
        assert!(state.submit_pin().is_none());
        state.open_pin(14);
        assert!(state.submit_pin().is_none());
    }

    #[test]
    fn firewall_help_tracks_copy_focus_and_clears_on_close() {
        let mut state = State::default();
        state.open_firewall_help(FirewallHelp {
            peer_ip: Some("192.0.2.10".into()),
            tcp_port: Some(49152),
            udp_port: None,
            subnet: None,
            copy_selected: true,
        });
        assert_eq!(
            state.prompt(),
            Some(Prompt::Firewall {
                copy_selected: true
            })
        );
        state.select_firewall_copy(false);
        assert_eq!(
            state.prompt(),
            Some(Prompt::Firewall {
                copy_selected: false
            })
        );
        state.close_pairing();
        assert!(state.prompt().is_none());
        assert!(state.firewall_help().is_none());
    }
}

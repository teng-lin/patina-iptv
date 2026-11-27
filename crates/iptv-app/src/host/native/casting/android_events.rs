//! Bounded callbacks for one Android foreground-service lease.
use iptv_core::cast::Command;
use tokio::sync::{mpsc, watch};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ServiceState {
    Pending,
    Ready,
    Stopped,
}

pub(super) struct HostEvents {
    pub(super) state: watch::Sender<ServiceState>,
    pub(super) commands: mpsc::Sender<Command>,
}

impl HostEvents {
    pub(super) fn notify(&self, event: i32) {
        match event {
            0 => {
                self.state.send_if_modified(|state| {
                    if *state != ServiceState::Pending {
                        return false;
                    }
                    *state = ServiceState::Ready;
                    true
                });
            }
            1 => {
                // Terminal events cannot be displaced by queued key repeats.
                self.state.send_replace(ServiceState::Stopped);
            }
            2 | 3 if *self.state.borrow() == ServiceState::Ready => {
                let command = if event == 2 {
                    Command::Play
                } else {
                    Command::Pause
                };
                let _ = self.commands.try_send(command);
            }
            _ => {}
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_stop_survives_a_full_command_queue_and_a_late_ready() {
        let (state, status) = watch::channel(ServiceState::Pending);
        let (commands, mut received) = mpsc::channel(1);
        let events = HostEvents { state, commands };

        events.notify(2);
        assert!(received.try_recv().is_err());
        events.notify(0);
        assert_eq!(*status.borrow(), ServiceState::Ready);
        events.notify(2);
        events.notify(3);
        events.notify(1);
        events.notify(0);
        assert_eq!(*status.borrow(), ServiceState::Stopped);
        assert_eq!(received.try_recv().unwrap(), Command::Play);
        events.notify(3);
        assert!(received.try_recv().is_err());
    }
}

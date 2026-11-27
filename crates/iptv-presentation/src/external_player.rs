//! External playback request identities and transfer targets.

use iptv_core::parental::Target;

#[derive(Clone, PartialEq, Eq)]
pub struct Handoff {
    pub ticket: u64,
    pub target: Target,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct State {
    request: u64,
    pending: Option<Handoff>,
    active: Option<Handoff>,
}

impl State {
    pub fn request(&self) -> u64 {
        self.request
    }

    /// Invalidate async authorization and give the next request its identity.
    pub fn supersede(&mut self) {
        self.request = self.request.wrapping_add(1);
    }

    pub fn mark_pending(&mut self, ticket: u64, target: Target) {
        if ticket == self.request {
            self.pending = Some(Handoff { ticket, target });
        }
    }

    pub fn pending_target(&self) -> Option<&Target> {
        self.pending
            .as_ref()
            .filter(|pending| pending.ticket == self.request)
            .map(|pending| &pending.target)
    }

    /// Recover the affected target even when the request has been invalidated.
    pub fn clear_pending(&mut self, ticket: Option<u64>) -> Option<Target> {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| ticket.is_none_or(|ticket| ticket == pending.ticket))
        {
            self.pending.take().map(|pending| pending.target)
        } else {
            None
        }
    }

    pub fn activate(&mut self, handoff: Handoff) {
        self.active = Some(handoff);
    }

    pub fn is_active(&self, ticket: u64) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.ticket == ticket)
    }

    pub fn clear_active(&mut self, ticket: u64) {
        if self.is_active(ticket) {
            self.active = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_handoff_cleanup_cannot_remove_its_replacement() {
        let mut state = State::default();
        let old = Target::live(1, "old");
        let new = Target::live(2, "new");
        state.mark_pending(0, old.clone());
        state.supersede();
        assert_eq!(state.pending_target(), None);
        assert_eq!(state.clear_pending(Some(0)), Some(old));
        state.mark_pending(1, new.clone());
        assert_eq!(state.clear_pending(Some(0)), None);
        state.activate(Handoff {
            ticket: 1,
            target: new.clone(),
        });
        state.clear_active(0);
        assert!(state.is_active(1));
        assert_eq!(state.pending_target(), Some(&new));
    }
}

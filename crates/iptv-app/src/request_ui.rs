//! NSH signals for modal state with asynchronous request epochs.

use iptv_presentation::request_state::State;
use nsh_core::prelude::{Memo, Signal, computed, signal, untracked};

pub(crate) struct RequestUi<T> {
    state: Signal<State<T>>,
    epoch: Memo<u64>,
}

impl<T> Copy for RequestUi<T> {}
impl<T> Clone for RequestUi<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Clone + PartialEq + 'static> RequestUi<T> {
    pub(crate) fn new() -> Self {
        let state = signal(State::<T>::default());
        Self {
            state,
            epoch: computed(move || state.with(State::epoch)),
        }
    }

    pub(crate) fn open(self, value: T) -> u64 {
        let mut epoch = 0;
        self.state.update(|state| epoch = state.open(value));
        epoch
    }

    pub(crate) fn close(self) -> bool {
        let mut closed = false;
        self.state.update(|state| closed = state.close());
        closed
    }

    pub(crate) fn read<R>(self, read: impl FnOnce(&T) -> R) -> Option<R> {
        self.state.with(|state| state.current.as_ref().map(read))
    }

    pub(crate) fn update(self, edit: impl FnOnce(&mut T)) {
        self.state.update(|state| {
            if let Some(current) = state.current.as_mut() {
                edit(current);
            }
        });
    }

    pub(crate) fn is_open(self) -> bool {
        self.state.with(|state| state.current.is_some())
    }

    pub(crate) fn epoch(self) -> u64 {
        self.epoch.get_untracked()
    }

    pub(crate) fn is_current(self, epoch: u64) -> bool {
        untracked(|| self.epoch.try_get()) == Some(epoch)
    }

    pub(crate) fn next_request(self) -> u64 {
        let mut epoch = 0;
        self.state.update(|state| epoch = state.next_request());
        epoch
    }
}

//! NSH signals for modal state with asynchronous request epochs.

use iptv_presentation::request_state::Epoch;
use nsh_core::prelude::{Signal, Stored, signal, stored};

pub(crate) struct RequestUi<T> {
    state: Signal<Option<T>>,
    // Retiring a reply does not change anything the dialog renders.
    epoch: Stored<Epoch>,
}

impl<T> Copy for RequestUi<T> {}
impl<T> Clone for RequestUi<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Clone + PartialEq + 'static> RequestUi<T> {
    pub(crate) fn new() -> Self {
        Self {
            state: signal(None),
            epoch: stored(Epoch::default()),
        }
    }

    pub(crate) fn open(self, value: T) -> u64 {
        let epoch = self.next_request();
        self.state.set(Some(value));
        epoch
    }

    pub(crate) fn close(self) -> bool {
        self.next_request();
        let was_open = self.state.with_untracked(Option::is_some);
        if was_open {
            self.state.set(None);
        }
        was_open
    }

    pub(crate) fn read<R>(self, read: impl FnOnce(&T) -> R) -> Option<R> {
        self.state.with(|state| state.as_ref().map(read))
    }

    pub(crate) fn update(self, edit: impl FnOnce(&mut T)) {
        self.state.update(|state| {
            if let Some(current) = state.as_mut() {
                edit(current);
            }
        });
    }

    pub(crate) fn is_open(self) -> bool {
        self.state.with(Option::is_some)
    }

    pub(crate) fn epoch(self) -> u64 {
        self.epoch.with(|epoch| epoch.current())
    }

    pub(crate) fn is_current(self, epoch: u64) -> bool {
        self.epoch.try_with(|current| current.current()) == Some(epoch)
    }

    pub(crate) fn next_request(self) -> u64 {
        self.epoch.with(Epoch::advance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsh_core::prelude::*;

    #[component]
    fn Dialogs() -> impl IntoElement {
        let original = RequestUi::new();
        let other = RequestUi::new();
        let first = original.open(1);
        let independent = other.open(1);
        let renders = signal(0);
        let fenced = signal(0);
        effect(move || {
            original.read(|_| ());
            untracked(|| renders.update(|count| *count += 1));
        });
        view()
            .child(text(move || format!("Content renders {}", renders.get())))
            .child(text(move || format!("Old reply fenced {}", fenced.get())))
            .child(view().label("Retire reply").h(px(40.0)).on_press(move || {
                original.next_request();
                fenced.set(i32::from(
                    !original.is_current(first) && other.is_current(independent),
                ));
            }))
            .child(view().label("Reopen").h(px(40.0)).on_press(move || {
                original.close();
                let latest = original.open(2);
                fenced.set(
                    2 * i32::from(
                        !original.is_current(first)
                            && other.is_current(independent)
                            && original.is_current(latest),
                    ),
                );
            }))
    }

    #[test]
    fn epochs_fence_replies_without_invalidating_visible_content_or_another_dialog() {
        let mut app = nsh_testing::Headless::start(nsh_engine::EngineConfig::new(
            nsh_core::element::Root::of::<Dialogs>(),
        ))
        .unwrap();
        app.wait_for("text:Content renders 1").unwrap();
        app.press("label:Retire reply").unwrap();
        app.wait_for("text:Old reply fenced 1").unwrap();
        assert!(app.screen().matches("text:Content renders 1").unwrap());
        app.press("label:Reopen").unwrap();
        app.wait_for("text:Old reply fenced 2").unwrap();
    }
}

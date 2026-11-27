//! Schedules accepted UI-thread tasks under the current app root.

use std::{cell::Cell, future::Future};

use nsh_core::prelude::*;

use crate::wiring::runtime_owner::in_runtime;

thread_local! {
    /// The app's root owner: accepted tasks outlive the view that started them.
    static APP_OWNER: Cell<Option<Owner>> = const { Cell::new(None) };
}

/// Sets the root owner used by detached tasks.
pub(crate) fn bind_owner() {
    APP_OWNER.set(Some(Owner::current()));
}

/// Runs `future` on the UI thread under the app's root owner.
pub(crate) fn spawn_in_app(future: impl Future<Output = ()> + 'static) {
    let owner = APP_OWNER.with(Cell::get).unwrap_or_else(Owner::current);
    let future = in_runtime(future);
    owner.run(move || {
        // Dropping the handle detaches the task; it lives with the owner.
        drop(spawn(future));
    });
}

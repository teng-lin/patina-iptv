//! Explicit worker acknowledgements for synthetic lifecycle scenarios only.
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Progress {
    entered: bool,
    released: bool,
}

#[derive(Default)]
pub(crate) struct GateState {
    progress: Mutex<Progress>,
    changed: Condvar,
}

impl GateState {
    pub(crate) fn acknowledge(&self) {
        self.progress.lock().unwrap().entered = true;
        self.changed.notify_all();
    }
    /// Called only after a worker has reached the boundary being exercised.
    pub(crate) fn pause(&self) {
        let mut progress = self.progress.lock().unwrap();
        progress.entered = true;
        self.changed.notify_all();
        while !progress.released {
            progress = self.changed.wait(progress).unwrap();
        }
    }
}

/// A test-owned release and an acknowledgement from the actual worker.
/// Dropping the gate releases it even when an assertion unwinds.
pub struct TestGate(pub(crate) Arc<GateState>);

impl TestGate {
    pub(crate) fn new() -> Self {
        Self(Arc::default())
    }

    /// Time only bounds a failed test; an elapsed timeout never means ready.
    pub fn wait_entered(&self, limit: Duration) -> bool {
        let progress = self.0.progress.lock().unwrap();
        self.0
            .changed
            .wait_timeout_while(progress, limit, |progress| !progress.entered)
            .unwrap()
            .0
            .entered
    }

    pub fn release(&self) {
        self.0.progress.lock().unwrap().released = true;
        self.0.changed.notify_all();
    }
}

impl Drop for TestGate {
    fn drop(&mut self) {
        self.release();
    }
}

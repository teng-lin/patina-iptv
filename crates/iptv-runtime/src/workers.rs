//! Bounded refresh execution. Work for the visible source precedes background work.
//!
//! The workers come from the owner's execution port. A host without workers
//! (a cooperative browser executor) gets a pool that refuses work, which
//! imports report as busy.
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

pub(crate) type Work = Box<dyn FnOnce() + Send>;
const PARALLEL: usize = 2;
const WAITING: usize = 32;

#[derive(Default)]
struct State {
    visible: VecDeque<Work>,
    background: VecDeque<Work>,
    closed: bool,
}
pub(crate) struct Pool {
    shared: Arc<(Mutex<State>, Condvar)>,
    /// Workers that started; with none, work is refused.
    workers: usize,
    capacity: usize,
}
impl Pool {
    fn new(execution: &dyn crate::execution::Execution, parallel: usize, capacity: usize) -> Self {
        let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let mut workers = 0;
        for index in 0..parallel {
            let shared = shared.clone();
            match execution.worker(
                format!("iptv-refresh-{index}"),
                Box::new(move || work(&shared)),
            ) {
                Ok(()) => workers += 1,
                Err(error) => log::error!("iptv refresh: cannot start a worker: {error}"),
            }
        }
        Self {
            shared,
            workers,
            capacity,
        }
    }
    /// The production pool: two refreshes at once, a bounded queue behind.
    pub(crate) fn standard(execution: &dyn crate::execution::Execution) -> Self {
        Self::new(execution, PARALLEL, WAITING)
    }
    pub(crate) fn submit(&self, visible: bool, work: Work) -> bool {
        let (lock, ready) = &*self.shared;
        let mut state = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.workers == 0
            || state.closed
            || state.visible.len() + state.background.len() >= self.capacity
        {
            return false;
        }
        if visible {
            state.visible.push_back(work);
        } else {
            state.background.push_back(work);
        }
        ready.notify_one();
        true
    }
}
impl Pool {
    pub(crate) fn close(&self) {
        let (lock, ready) = &*self.shared;
        let mut state = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.closed = true;
        state.visible.clear();
        state.background.clear();
        ready.notify_all();
    }
}
impl Drop for Pool {
    fn drop(&mut self) {
        self.close();
    }
}

/// One worker: runs queued work, the visible source's first, until the
/// pool closes.
fn work(shared: &(Mutex<State>, Condvar)) {
    loop {
        let work = {
            let (lock, ready) = shared;
            let mut state = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            loop {
                if state.closed {
                    return;
                }
                if let Some(work) = state
                    .visible
                    .pop_front()
                    .or_else(|| state.background.pop_front())
                {
                    break work;
                }
                state = ready
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        };
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).is_err() {
            log::error!("iptv refresh: task panicked; worker remains available");
        }
    }
}

/// Synthetic conditions of one owner's workers, for the headless tests: a
/// queue that refuses work, and an import held just before it reports.
#[cfg(feature = "test-support")]
#[derive(Default)]
pub(crate) struct Hooks {
    refusals: std::sync::atomic::AtomicUsize,
    reports: Mutex<(bool, usize)>,
    released: Condvar,
    publication: Mutex<Option<Arc<crate::testing::GateState>>>,
    source_preparation: Mutex<Option<Arc<crate::testing::GateState>>>,
    drained: Mutex<Option<Arc<crate::testing::GateState>>>,
    admission_closed: Mutex<Option<Arc<crate::testing::GateState>>>,
    drain_limit: Mutex<Option<std::time::Duration>>,
}
#[cfg(feature = "test-support")]
impl Hooks {
    pub(crate) fn hold_source_preparation(&self) -> crate::TestGate {
        let gate = crate::TestGate::new();
        *self.source_preparation.lock().unwrap() = Some(gate.0.clone());
        gate
    }
    #[cfg(feature = "native")]
    pub(crate) fn take_source_preparation(&self) -> Option<Arc<crate::testing::GateState>> {
        self.source_preparation.lock().unwrap().take()
    }
    pub(crate) fn observe_admission_close(&self) -> crate::TestGate {
        let gate = crate::TestGate::new();
        *self.admission_closed.lock().unwrap() = Some(gate.0.clone());
        gate
    }
    pub(crate) fn admission_closed(&self) {
        if let Some(gate) = self.admission_closed.lock().unwrap().take() {
            gate.acknowledge();
        }
    }
    #[cfg(test)]
    pub(crate) fn set_drain_limit(&self, limit: Option<std::time::Duration>) {
        *self.drain_limit.lock().unwrap() = limit;
    }
    pub(crate) fn drain_limit(&self) -> Option<std::time::Duration> {
        *self.drain_limit.lock().unwrap()
    }
    pub(crate) fn observe_drain(&self) -> crate::TestGate {
        let gate = crate::TestGate::new();
        *self.drained.lock().unwrap() = Some(gate.0.clone());
        gate
    }
    pub(crate) fn drained(&self) {
        if let Some(gate) = self.drained.lock().unwrap().take() {
            gate.acknowledge();
        }
    }
    pub(crate) fn hold_publication(&self) -> crate::TestGate {
        let gate = crate::TestGate::new();
        *self.publication.lock().unwrap() = Some(gate.0.clone());
        gate
    }
    pub(crate) fn before_publication(&self) {
        let pause = self.publication.lock().unwrap().take();
        if let Some(pause) = pause {
            pause.pause();
        }
    }
    pub(crate) fn refuse(&self) -> bool {
        use std::sync::atomic::Ordering::SeqCst;
        self.refusals
            .try_update(SeqCst, SeqCst, |left| left.checked_sub(1))
            .is_ok()
    }
    fn reports(&self) -> std::sync::MutexGuard<'_, (bool, usize)> {
        self.reports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub(crate) fn refuse_work(&self, count: usize) {
        self.refusals
            .store(count, std::sync::atomic::Ordering::SeqCst);
    }
    pub(crate) fn hold_reports(&self, held: bool) {
        if held {
            self.reports().0 = true;
        } else {
            self.release();
        }
    }
    pub(crate) fn reports_waiting(&self) -> usize {
        self.reports().1
    }
    pub(crate) fn before_report(&self) {
        let mut reports = self.reports();
        reports.1 += 1;
        while reports.0 {
            reports = self
                .released
                .wait(reports)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        reports.1 -= 1;
    }
    pub(crate) fn release(&self) {
        self.reports().0 = false;
        self.released.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn pool(parallel: usize, capacity: usize) -> Pool {
        Pool::new(
            &crate::native::execution::NativeExecution,
            parallel,
            capacity,
        )
    }

    #[test]
    fn a_host_without_workers_refuses_refresh_work() {
        struct Idle;
        impl crate::execution::CooperativeHost for Idle {
            fn spawn(&self, _: crate::execution::Task) {}
            fn delay(&self, _: Duration) -> crate::execution::Task {
                Box::pin(std::future::pending())
            }
            fn now(&self) -> Duration {
                Duration::ZERO
            }
        }
        let execution = crate::execution::Cooperative::new(Arc::new(Idle));
        let pool = Pool::standard(&execution);
        assert!(!pool.submit(true, Box::new(|| panic!("no worker runs refused work"))));
    }

    #[test]
    fn a_panicking_source_does_not_remove_a_worker() {
        let pool = pool(1, 2);
        let (tx, rx) = mpsc::channel();
        assert!(pool.submit(false, Box::new(|| panic!("synthetic refresh failure"))));
        assert!(pool.submit(
            false,
            Box::new(move || {
                tx.send(()).unwrap();
            })
        ));
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn visible_source_runs_before_queued_background_and_queue_is_bounded() {
        let pool = pool(1, 2);
        let (started, running) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        assert!(pool.submit(
            false,
            Box::new(move || {
                started.send(()).unwrap();
                blocked.recv().unwrap();
            })
        ));
        running.recv_timeout(Duration::from_secs(5)).unwrap();
        let (order, received) = mpsc::channel();
        let background = order.clone();
        assert!(pool.submit(
            false,
            Box::new(move || background.send("background").unwrap())
        ));
        assert!(pool.submit(true, Box::new(move || order.send("visible").unwrap())));
        assert!(!pool.submit(true, Box::new(|| panic!("rejected work must not run"))));
        release.send(()).unwrap();
        assert_eq!(
            received.recv_timeout(Duration::from_secs(5)).unwrap(),
            "visible"
        );
        assert_eq!(
            received.recv_timeout(Duration::from_secs(5)).unwrap(),
            "background"
        );
    }

    #[test]
    fn many_sources_share_two_workers() {
        let pool = pool(2, 32);
        let (started, received) = mpsc::channel();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        for source in 0..20 {
            let started = started.clone();
            let gate = gate.clone();
            assert!(pool.submit(
                false,
                Box::new(move || {
                    started.send(source).unwrap();
                    let (lock, ready) = &*gate;
                    let mut released = lock.lock().unwrap();
                    while !*released {
                        released = ready.wait(released).unwrap();
                    }
                })
            ));
        }
        received.recv_timeout(Duration::from_secs(5)).unwrap();
        received.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(received.recv_timeout(Duration::from_millis(100)).is_err());
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        for _ in 2..20 {
            received.recv_timeout(Duration::from_secs(5)).unwrap();
        }
    }
}

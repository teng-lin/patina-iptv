//! Bounded refresh execution. Work for the visible source precedes background work.
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

type Work = Box<dyn FnOnce() + Send>;
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
    workers: Vec<JoinHandle<()>>,
    capacity: usize,
}
impl Pool {
    fn new(parallel: usize, capacity: usize) -> Self {
        let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let workers = (0..parallel)
            .map(|index| {
                let shared = shared.clone();
                std::thread::Builder::new()
                    .name(format!("iptv-refresh-{index}"))
                    .spawn(move || {
                        loop {
                            let work = {
                                let (lock, ready) = &*shared;
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
                            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).is_err()
                            {
                                log::error!(
                                    "iptv refresh: task panicked; worker remains available"
                                );
                            }
                        }
                    })
                    .expect("start refresh worker")
            })
            .collect();
        Self {
            shared,
            workers,
            capacity,
        }
    }
    fn submit(&self, visible: bool, work: Work) -> bool {
        let (lock, ready) = &*self.shared;
        let mut state = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed || state.visible.len() + state.background.len() >= self.capacity {
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
        self.workers.clear();
    }
}

pub fn submit(visible: bool, work: impl FnOnce() + Send + 'static) -> bool {
    let work = super::store_service::scoped(move || {
        // Retired queued refreshes must not spend a new app's network budget.
        // Running work remains guarded again at every store operation.
        if super::store_service::is_current() {
            work();
        }
    });
    super::store_service::service()
        .pool
        .get_or_init(|| Pool::new(PARALLEL, WAITING))
        .submit(visible, Box::new(work))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn a_panicking_source_does_not_remove_a_worker() {
        let pool = Pool::new(1, 2);
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
        let pool = Pool::new(1, 2);
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
        let pool = Pool::new(2, 32);
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

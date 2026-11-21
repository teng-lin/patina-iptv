//! The browser's execution: everything on the owner's thread.
//!
//! A browser host runs one thread (the Web Worker the app lives in) and an
//! event loop that polls its tasks after each delivery: a key, a network
//! chunk, a timer. [`Cooperative`] fits the runtime's work into that loop:
//!
//! - A lane is a queue that runs as a host task. Each turn runs queued jobs
//!   until the turn's budget is spent, then yields to the host's timer, so
//!   a long queue does not hold the page. A [`Sliced`] job (a resumable
//!   publish) runs one step per turn, within the turn's remaining budget.
//! - [`Lane::run`] (a blocking request) runs the lane's queue inline, up to
//!   and including its job: one thread cannot wait for another, and running
//!   the queue in order gives the same answer a waiting caller would get.
//! - Tasks are the host's tasks; CPU-bound work runs as a task too.
//! - There are no workers. Services built on them (the refresh pool, the
//!   file and favorites writers) report themselves unavailable.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use super::{Budget, Execution, Job, Lane, LocalTask, Progress, Sliced, Task, Tasks};

/// The host's single-threaded event loop, as a [`Cooperative`] executor
/// uses it.
pub trait CooperativeHost: Send + Sync {
    /// Runs `task` from the event loop's next poll on.
    fn spawn(&self, task: Task);
    /// A future that ends after `duration`, on the host's timer.
    fn delay(&self, duration: Duration) -> Task;
    /// The host's monotonic clock (turn budgets).
    fn now(&self) -> Duration;
}

/// Runs the runtime's work on the owner's thread, in turns of the host's
/// event loop.
#[derive(Clone)]
pub struct Cooperative {
    host: Arc<dyn CooperativeHost>,
    turn: Duration,
}

impl Cooperative {
    /// A lane's turn: on a TV, work of about this length leaves the page
    /// time to draw and to take the next key.
    pub const TURN: Duration = Duration::from_millis(40);

    pub fn new(host: Arc<dyn CooperativeHost>) -> Self {
        Self {
            host,
            turn: Self::TURN,
        }
    }

    /// With turns of `turn` instead of [`Self::TURN`].
    pub fn with_turn(mut self, turn: Duration) -> Self {
        self.turn = turn;
        self
    }
}

impl Execution for Cooperative {
    fn lane(&self, _name: &'static str) -> io::Result<Box<dyn Lane>> {
        Ok(Box::new(TurnLane {
            state: Arc::default(),
            host: self.host.clone(),
            turn: self.turn,
        }))
    }

    fn worker(&self, _name: String, _body: Job) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "a cooperative executor has no workers",
        ))
    }

    fn tasks(&self) -> Box<dyn Tasks> {
        Box::new(HostTasks(self.host.clone()))
    }
}

enum Item {
    Job(Job),
    Sliced(Box<dyn Sliced>),
}

#[derive(Default)]
struct State {
    queue: VecDeque<Item>,
    /// A job of this lane is running now.
    running: bool,
    /// A turn is spawned or running.
    scheduled: bool,
}

struct TurnLane {
    state: Arc<Mutex<State>>,
    host: Arc<dyn CooperativeHost>,
    turn: Duration,
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    // Jobs run outside the lock, so a panicking job cannot leave it half
    // updated.
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

impl TurnLane {
    fn enqueue(&self, item: Item) -> bool {
        let schedule = {
            let mut state = lock(&self.state);
            state.queue.push_back(item);
            !std::mem::replace(&mut state.scheduled, true)
        };
        if schedule {
            let state = self.state.clone();
            let host = self.host.clone();
            let turn = self.turn;
            self.host.spawn(Box::pin(async move {
                while run_turn(&state, &*host, turn) {
                    // Yield through the host's timer: a task that is only
                    // pending is polled again at the next delivery, which
                    // may be a long time away.
                    host.delay(Duration::ZERO).await;
                }
            }));
        }
        true
    }
}

/// Runs queued work until `turn` is spent; true when work is left.
fn run_turn(state: &Mutex<State>, host: &dyn CooperativeHost, turn: Duration) -> bool {
    let started = host.now();
    loop {
        let spent = host.now().saturating_sub(started);
        let Some(left) = turn.checked_sub(spent).filter(|left| !left.is_zero()) else {
            return true;
        };
        let Some(item) = begin(state) else {
            lock(state).scheduled = false;
            return false;
        };
        match item {
            Item::Job(job) => {
                job();
                lock(state).running = false;
            }
            Item::Sliced(mut job) => {
                let progress = job.step(Budget { time: Some(left) });
                let mut state = lock(state);
                state.running = false;
                if progress == Progress::Pending {
                    // Still the oldest work: nothing queued after it runs first.
                    state.queue.push_front(Item::Sliced(job));
                    return true;
                }
            }
        }
    }
}

/// The next item, marked running; `None` when the queue is empty.
fn begin(state: &Mutex<State>) -> Option<Item> {
    let mut state = lock(state);
    let item = state.queue.pop_front()?;
    state.running = true;
    Some(item)
}

impl Lane for TurnLane {
    fn push(&self, job: Job) -> bool {
        self.enqueue(Item::Job(job))
    }

    fn push_sliced(&self, job: Box<dyn Sliced>) -> bool {
        self.enqueue(Item::Sliced(job))
    }

    fn run(&self, job: Job, _limit: Option<Duration>) -> bool {
        if lock(&self.state).running {
            // A job of this lane waiting for its own lane.
            return false;
        }
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ran = done.clone();
        self.push(Box::new(move || {
            job();
            ran.store(true, std::sync::atomic::Ordering::Release);
        }));
        // Everything queued before it runs first, in order; a sliced job
        // runs to its end, as it would before a waiting caller's job.
        while !done.load(std::sync::atomic::Ordering::Acquire) {
            match begin(&self.state) {
                Some(Item::Job(job)) => job(),
                Some(Item::Sliced(mut job)) => {
                    while job.step(Budget::UNLIMITED) == Progress::Pending {}
                }
                None => return false,
            }
            lock(&self.state).running = false;
        }
        true
    }

    fn is_current(&self) -> bool {
        lock(&self.state).running
    }
}

struct HostTasks(Arc<dyn CooperativeHost>);

impl Tasks for HostTasks {
    fn spawn(&self, task: Task) {
        self.0.spawn(task);
    }

    fn spawn_blocking(&self, work: Job) {
        // No thread to move it to: it runs as a task of its own, after
        // whatever is running now.
        self.0.spawn(Box::pin(async move { work() }));
    }

    fn block_on(&self, future: LocalTask<'_>) {
        // One thread cannot wait: a future that needs the event loop (a
        // network reply, a timer, a lane turn) would never end here. Work
        // that blocks belongs on workers, which a cooperative host lacks,
        // so reaching this is a bug in the caller, not a passing condition.
        let mut future = future;
        let mut context = Context::from_waker(Waker::noop());
        assert!(
            future.as_mut().poll(&mut context) == Poll::Ready(()),
            "a cooperative executor cannot block on pending work"
        );
    }

    fn delay(&self, duration: Duration) -> Task {
        self.0.delay(duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A synthetic event loop: spawned tasks wait until `poll`, and the
    /// clock moves only when a job advances it.
    #[derive(Default)]
    struct Loop {
        tasks: Mutex<Vec<Task>>,
        now_ms: AtomicU64,
        delays: AtomicU64,
    }

    impl CooperativeHost for Loop {
        fn spawn(&self, task: Task) {
            self.tasks.lock().unwrap().push(task);
        }
        fn delay(&self, _duration: Duration) -> Task {
            self.delays.fetch_add(1, Ordering::SeqCst);
            // Ends at the next poll: one turn of the loop.
            let mut first = true;
            Box::pin(std::future::poll_fn(move |_| {
                if std::mem::take(&mut first) {
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            }))
        }
        fn now(&self) -> Duration {
            Duration::from_millis(self.now_ms.load(Ordering::SeqCst))
        }
    }

    impl Loop {
        /// One delivery: polls every task once; false when none is left.
        fn poll(&self) -> bool {
            let mut tasks = std::mem::take(&mut *self.tasks.lock().unwrap());
            let mut context = Context::from_waker(Waker::noop());
            tasks.retain_mut(|task| task.as_mut().poll(&mut context).is_pending());
            let mut queued = self.tasks.lock().unwrap();
            tasks.append(&mut queued);
            *queued = tasks;
            !queued.is_empty()
        }
        fn advance(&self, ms: u64) {
            self.now_ms.fetch_add(ms, Ordering::SeqCst);
        }
    }

    fn setup() -> (Arc<Loop>, Box<dyn Lane>) {
        let host = Arc::new(Loop::default());
        let lane = Cooperative::new(host.clone())
            .with_turn(Duration::from_millis(10))
            .lane("synthetic")
            .unwrap();
        (host, lane)
    }

    #[test]
    fn jobs_run_in_order_in_turns_of_the_host_loop() {
        let (host, lane) = setup();
        let order = Arc::new(Mutex::new(Vec::new()));
        for index in 0..4 {
            let order = order.clone();
            let clock = host.clone();
            assert!(lane.push(Box::new(move || {
                order.lock().unwrap().push(index);
                clock.advance(6);
            })));
        }
        assert!(
            order.lock().unwrap().is_empty(),
            "nothing runs before a poll"
        );
        assert!(host.poll());
        // Two jobs spend the 10 ms turn; the lane yields to the host timer.
        assert_eq!(*order.lock().unwrap(), [0, 1]);
        assert!(host.poll());
        assert!(!host.poll());
        assert_eq!(*order.lock().unwrap(), [0, 1, 2, 3]);
    }

    #[test]
    fn a_blocking_request_runs_the_queue_before_it_inline() {
        let (host, lane) = setup();
        let order = Arc::new(Mutex::new(Vec::new()));
        let first = order.clone();
        lane.push(Box::new(move || first.lock().unwrap().push("queued")));
        let second = order.clone();
        assert!(lane.run(
            Box::new(move || second.lock().unwrap().push("waited")),
            None
        ));
        assert_eq!(*order.lock().unwrap(), ["queued", "waited"]);
        // The turn spawned by the first push finds nothing left.
        assert!(!host.poll());
    }

    struct Publish {
        left: u32,
        steps: Arc<Mutex<Vec<Budget>>>,
        host: Arc<Loop>,
    }

    impl Sliced for Publish {
        fn step(&mut self, budget: Budget) -> Progress {
            self.steps.lock().unwrap().push(budget);
            self.host.advance(4);
            self.left -= 1;
            if self.left == 0 {
                Progress::Done
            } else {
                Progress::Pending
            }
        }
    }

    #[test]
    fn a_sliced_job_runs_one_step_per_turn_and_keeps_its_place() {
        let (host, lane) = setup();
        let steps = Arc::new(Mutex::new(Vec::new()));
        assert!(lane.push_sliced(Box::new(Publish {
            left: 3,
            steps: steps.clone(),
            host: host.clone(),
        })));
        let after = Arc::new(Mutex::new(false));
        let flag = after.clone();
        lane.push(Box::new(move || *flag.lock().unwrap() = true));
        assert!(host.poll());
        assert_eq!(
            *steps.lock().unwrap(),
            [Budget {
                time: Some(Duration::from_millis(10))
            }]
        );
        assert!(
            !*after.lock().unwrap(),
            "work queued behind a publish waits"
        );
        assert!(host.poll());
        // The last step leaves time in its turn for the job behind it.
        assert!(!host.poll());
        assert_eq!(steps.lock().unwrap().len(), 3);
        assert!(*after.lock().unwrap());
    }

    #[test]
    fn a_blocking_request_finishes_a_sliced_job_queued_before_it() {
        let (host, lane) = setup();
        let steps = Arc::new(Mutex::new(Vec::new()));
        lane.push_sliced(Box::new(Publish {
            left: 2,
            steps: steps.clone(),
            host: host.clone(),
        }));
        assert!(lane.run(Box::new(|| {}), None));
        assert_eq!(*steps.lock().unwrap(), [Budget::UNLIMITED; 2]);
    }

    #[test]
    fn a_job_cannot_wait_for_its_own_lane_and_there_are_no_workers() {
        let host = Arc::new(Loop::default());
        let execution = Cooperative::new(host.clone());
        let lane: Arc<dyn Lane> = Arc::from(execution.lane("synthetic").unwrap());
        let inner = lane.clone();
        let answer = Arc::new(Mutex::new(None));
        let seen = answer.clone();
        assert!(lane.run(
            Box::new(move || {
                *seen.lock().unwrap() =
                    Some((inner.is_current(), inner.run(Box::new(|| {}), None)));
            }),
            None
        ));
        assert_eq!(*answer.lock().unwrap(), Some((true, false)));
        assert!(!lane.is_current());
        assert_eq!(
            execution
                .worker("synthetic".into(), Box::new(|| {}))
                .unwrap_err()
                .kind(),
            io::ErrorKind::Unsupported
        );
    }
}

//! The browser's execution: everything on the owner's thread.
//!
//! A browser host runs one thread (the Web Worker the app lives in) and an
//! event loop that polls its tasks after each delivery: a key, a network
//! chunk, a timer. [`Cooperative`] fits the runtime's work into that loop:
//!
//! - All of one executor's lanes run from one turn task. A turn runs queued
//!   work, one item per lane in turn, until the turn's time is spent
//!   (shared by every lane), then yields through the host's timer, so a
//!   long queue does not hold the page and one lane cannot starve another.
//! - A [`Sliced`] job (a resumable publish) runs one step of
//!   [`Cooperative::SLICE`] rows per visit; the other lanes' jobs (reads)
//!   run between its steps, and its own lane's later jobs wait for it.
//! - [`Lane::run`] (a blocking request) runs its lane's queue inline, up to
//!   and including its job: one thread cannot wait for another. A sliced job
//!   in front is stepped too, in slices; with a limit, only until the limit
//!   passes, and the request then reports that it did not run in time (it
//!   still runs later, in order).
//! - Lanes share one connection ([`Lane::shares_connection`]): a browser
//!   store has a single one, and reads run on it between writes.
//! - Tasks are the host's tasks; CPU-bound work runs as a task too.
//! - There are no workers. Services built on them (the refresh pool, the
//!   file and favorites writers) report themselves unavailable.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
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
    /// The host's monotonic clock (turn budgets, request limits).
    fn now(&self) -> Duration;
}

/// Runs the runtime's work on the owner's thread, in turns of the host's
/// event loop.
#[derive(Clone)]
pub struct Cooperative(Arc<Inner>);

struct Inner {
    host: Arc<dyn CooperativeHost>,
    turn: Duration,
    lanes: Mutex<Vec<Weak<Mutex<State>>>>,
    /// The turn task is spawned or running.
    scheduled: Mutex<bool>,
    /// The lane the next turn starts with: the one the last turn reached.
    next: Mutex<usize>,
}

impl Cooperative {
    /// A turn: on a TV, work of about this length leaves the page time to
    /// draw and to take the next key.
    pub const TURN: Duration = Duration::from_millis(40);

    /// Rows a sliced job's step does: about 50 ms of guide publishing on a
    /// 2020 TV (the plan's measurement).
    pub const SLICE: Budget = Budget { rows: 400 };

    pub fn new(host: Arc<dyn CooperativeHost>) -> Self {
        Self::with_turn(host, Self::TURN)
    }

    /// With turns of `turn` instead of [`Self::TURN`].
    pub fn with_turn(host: Arc<dyn CooperativeHost>, turn: Duration) -> Self {
        Self(Arc::new(Inner {
            host,
            turn,
            lanes: Mutex::default(),
            scheduled: Mutex::new(false),
            next: Mutex::new(0),
        }))
    }
}

impl Execution for Cooperative {
    fn lane(&self, _name: &'static str) -> io::Result<Box<dyn Lane>> {
        let state = Arc::new(Mutex::new(State::default()));
        lock(&self.0.lanes).push(Arc::downgrade(&state));
        Ok(Box::new(TurnLane {
            state,
            inner: self.0.clone(),
        }))
    }

    fn worker(&self, _name: String, _body: Job) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "a cooperative executor has no workers",
        ))
    }

    fn tasks(&self) -> Box<dyn Tasks> {
        Box::new(HostTasks(self.0.host.clone()))
    }

    fn shares_connection(&self) -> bool {
        true
    }

    fn has_workers(&self) -> bool {
        false
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
}

fn lock<T>(state: &Mutex<T>) -> MutexGuard<'_, T> {
    // Jobs run outside the locks, so a panicking job cannot leave them half
    // updated.
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Marks a lane running while one of its items runs, and clears it however
/// the item ends (also when it panics).
struct Running<'a>(&'a Mutex<State>);

impl Drop for Running<'_> {
    fn drop(&mut self) {
        lock(self.0).running = false;
    }
}

/// The next item of `state`, marked running; `None` when it has none, or
/// when one of its jobs is running already (a job waiting for its own lane).
fn begin(state: &Mutex<State>) -> Option<(Item, Running<'_>)> {
    let mut locked = lock(state);
    if locked.running {
        return None;
    }
    let item = locked.queue.pop_front()?;
    locked.running = true;
    Some((item, Running(state)))
}

/// Runs one item of `state`: a job, or one step of a sliced job, which
/// keeps its place at the front while it has more to do.
fn run_one(state: &Mutex<State>, budget: Budget) -> bool {
    let Some((item, running)) = begin(state) else {
        return false;
    };
    match item {
        Item::Job(job) => job(),
        Item::Sliced(mut job) => {
            let progress =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job.step(budget)));
            match progress {
                Ok(Progress::Pending) => lock(state).queue.push_front(Item::Sliced(job)),
                Ok(Progress::Done) => {}
                Err(_) => log::error!("iptv execution: a sliced job panicked; it was dropped"),
            }
        }
    }
    drop(running);
    true
}

impl Inner {
    fn lanes(&self) -> Vec<Arc<Mutex<State>>> {
        let mut lanes = lock(&self.lanes);
        lanes.retain(|lane| lane.strong_count() > 0);
        lanes.iter().filter_map(Weak::upgrade).collect()
    }

    fn has_work(&self) -> bool {
        self.lanes().iter().any(|lane| !lock(lane).queue.is_empty())
    }

    /// Spawns the turn task unless it is spawned already.
    fn schedule(self: &Arc<Self>) {
        if std::mem::replace(&mut *lock(&self.scheduled), true) {
            return;
        }
        let inner = self.clone();
        self.host.spawn(Box::pin(async move {
            while inner.run_turn() {
                // Yield through the host's timer: a task that is only
                // pending is polled again at the next delivery, which may
                // be a long time away.
                inner.host.delay(Duration::ZERO).await;
            }
        }));
    }

    /// Runs queued work, one item per lane in turn, until the turn's time
    /// is spent; true when work is left for another turn.
    fn run_turn(&self) -> bool {
        let started = self.host.now();
        let lanes = self.lanes();
        let mut next = *lock(&self.next);
        loop {
            let mut ran = false;
            let mut spent = false;
            for _ in 0..lanes.len() {
                if self.host.now().saturating_sub(started) >= self.turn {
                    spent = true;
                    break;
                }
                let lane = &lanes[next % lanes.len()];
                next = (next + 1) % lanes.len();
                ran |= run_one(lane, Cooperative::SLICE);
            }
            spent |= self.host.now().saturating_sub(started) >= self.turn;
            if !ran || spent {
                *lock(&self.next) = next;
                // Decide under the lock that ends the task, so a push now
                // spawns a new one instead of finding it still scheduled.
                let mut scheduled = lock(&self.scheduled);
                let more = self.has_work();
                *scheduled = more;
                return more;
            }
        }
    }
}

struct TurnLane {
    state: Arc<Mutex<State>>,
    inner: Arc<Inner>,
}

impl TurnLane {
    fn enqueue(&self, item: Item) -> bool {
        lock(&self.state).queue.push_back(item);
        self.inner.schedule();
        true
    }
}

impl Lane for TurnLane {
    fn push(&self, job: Job) -> bool {
        self.enqueue(Item::Job(job))
    }

    fn push_sliced(&self, job: Box<dyn Sliced>) -> bool {
        self.enqueue(Item::Sliced(job))
    }

    fn run(&self, job: Job, limit: Option<Duration>) -> bool {
        if lock(&self.state).running {
            // A job of this lane waiting for its own lane.
            return false;
        }
        let deadline = limit.map(|limit| self.inner.host.now() + limit);
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ran = done.clone();
        self.push(Box::new(move || {
            job();
            ran.store(true, std::sync::atomic::Ordering::Release);
        }));
        // Everything queued before it runs first, in order, a sliced job
        // in slices.
        while !done.load(std::sync::atomic::Ordering::Acquire) {
            if deadline.is_some_and(|deadline| self.inner.host.now() >= deadline) {
                return false;
            }
            if !run_one(&self.state, Cooperative::SLICE) {
                return false;
            }
        }
        true
    }

    fn cancel_sliced(&self) {
        let cancelled: Vec<_> = {
            let mut state = lock(&self.state);
            let queue = std::mem::take(&mut state.queue);
            let (sliced, kept): (Vec<_>, Vec<_>) = queue
                .into_iter()
                .partition(|item| matches!(item, Item::Sliced(_)));
            state.queue = kept.into();
            sliced
        };
        for item in cancelled {
            if let Item::Sliced(job) = item {
                job.cancel();
            }
        }
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
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    /// A task kept by the synthetic loop. Without the native feature tasks
    /// are not `Send`; the loop runs every one on the test's own thread.
    struct Held(Task);
    // SAFETY: a `Held` is made, polled and dropped on the thread that runs
    // its test; the loop's mutex is only taken there.
    unsafe impl Send for Held {}

    /// A synthetic event loop like iptv-web's: spawned tasks wait until
    /// `poll`, which runs rounds while a task was woken, and the clock moves
    /// only when a job advances it.
    #[derive(Default)]
    struct Loop {
        tasks: Mutex<Vec<Held>>,
        now_ms: AtomicU64,
        woken: Arc<AtomicBool>,
    }

    struct Woken(Arc<AtomicBool>);
    impl std::task::Wake for Woken {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    impl CooperativeHost for Loop {
        fn spawn(&self, task: Task) {
            self.tasks.lock().unwrap().push(Held(task));
            self.woken.store(true, Ordering::SeqCst);
        }
        fn delay(&self, _duration: Duration) -> Task {
            // Ends at the next delivery: one turn of the loop.
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
        /// One delivery: rounds of polls while a task was woken or spawned;
        /// false when no task is left.
        fn poll(&self) -> bool {
            let waker = Waker::from(Arc::new(Woken(self.woken.clone())));
            let mut context = Context::from_waker(&waker);
            loop {
                self.woken.store(false, Ordering::SeqCst);
                let mut tasks = std::mem::take(&mut *self.tasks.lock().unwrap());
                tasks.retain_mut(|task| task.0.as_mut().poll(&mut context).is_pending());
                self.tasks.lock().unwrap().extend(tasks);
                if !self.woken.load(Ordering::SeqCst) {
                    break;
                }
            }
            !self.tasks.lock().unwrap().is_empty()
        }
        fn advance(&self, ms: u64) {
            self.now_ms.fetch_add(ms, Ordering::SeqCst);
        }
    }

    fn setup() -> (Arc<Loop>, Cooperative) {
        let host = Arc::new(Loop::default());
        let execution = Cooperative::with_turn(host.clone(), Duration::from_millis(10));
        (host, execution)
    }

    #[test]
    fn jobs_run_in_order_in_turns_of_the_host_loop() {
        let (host, execution) = setup();
        let lane = execution.lane("synthetic").unwrap();
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
        // Two jobs spend the 10 ms turn; the lanes yield to the host timer.
        assert_eq!(*order.lock().unwrap(), [0, 1]);
        // The last job spends the turn and leaves nothing: no further turn.
        assert!(!host.poll());
        assert_eq!(*order.lock().unwrap(), [0, 1, 2, 3]);
    }

    #[test]
    fn lanes_share_one_turn_and_take_turns() {
        let (host, execution) = setup();
        let writer = execution.lane("writer").unwrap();
        let reader = execution.lane("reader").unwrap();
        let order = Arc::new(Mutex::new(Vec::new()));
        for (lane, name) in [(&writer, "w"), (&reader, "r")] {
            for _ in 0..3 {
                let order = order.clone();
                let clock = host.clone();
                lane.push(Box::new(move || {
                    order.lock().unwrap().push(name);
                    clock.advance(4);
                }));
            }
        }
        assert!(host.poll());
        // 10 ms for both lanes together, alternating: not 10 ms each.
        assert_eq!(*order.lock().unwrap(), ["w", "r", "w"]);
        assert!(!host.poll());
        assert_eq!(*order.lock().unwrap(), ["w", "r", "w", "r", "w", "r"]);
    }

    #[test]
    fn a_task_waiting_for_a_lane_reply_runs_in_the_same_delivery() {
        let (host, execution) = setup();
        let lane = execution.lane("synthetic").unwrap();
        let (reply, answer) = futures_channel::oneshot::channel();
        let done = Arc::new(AtomicBool::new(false));
        let seen = done.clone();
        host.spawn(Box::pin(async move {
            assert_eq!(answer.await, Ok(7));
            seen.store(true, Ordering::SeqCst);
        }));
        lane.push(Box::new(move || {
            let _ = reply.send(7);
        }));
        assert!(!host.poll());
        assert!(done.load(Ordering::SeqCst));
    }

    #[test]
    fn a_blocking_request_runs_the_queue_before_it_inline() {
        let (host, execution) = setup();
        let lane = execution.lane("synthetic").unwrap();
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
        cancelled: Arc<AtomicBool>,
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
        fn cancel(self: Box<Self>) {
            self.cancelled.store(true, Ordering::SeqCst);
        }
    }

    fn publish(
        host: &Arc<Loop>,
        left: u32,
    ) -> (Box<Publish>, Arc<Mutex<Vec<Budget>>>, Arc<AtomicBool>) {
        let steps = Arc::new(Mutex::new(Vec::new()));
        let cancelled = Arc::new(AtomicBool::new(false));
        let job = Box::new(Publish {
            left,
            steps: steps.clone(),
            cancelled: cancelled.clone(),
            host: host.clone(),
        });
        (job, steps, cancelled)
    }

    #[test]
    fn a_sliced_job_runs_in_row_slices_with_reads_between_them() {
        let (host, execution) = setup();
        let writer = execution.lane("writer").unwrap();
        let reader = execution.lane("reader").unwrap();
        let (job, steps, _) = publish(&host, 3);
        assert!(writer.push_sliced(job));
        let log = Arc::new(Mutex::new(Vec::new()));
        let write = log.clone();
        writer.push(Box::new(move || write.lock().unwrap().push("write")));
        for _ in 0..2 {
            let read = log.clone();
            let counted = steps.clone();
            reader.push(Box::new(move || {
                let at = counted.lock().unwrap().len();
                read.lock().unwrap().push(if at == 1 {
                    "read after 1"
                } else {
                    "read after 2"
                });
            }));
        }
        assert!(host.poll());
        assert_eq!(*steps.lock().unwrap(), [Cooperative::SLICE; 3]);
        assert_eq!(*log.lock().unwrap(), ["read after 1", "read after 2"]);
        // The write queued behind the publish waited for all of it.
        assert!(!host.poll());
        assert_eq!(
            *log.lock().unwrap(),
            ["read after 1", "read after 2", "write"]
        );
    }

    #[test]
    fn a_timed_blocking_request_steps_a_publish_only_until_its_limit() {
        let (host, execution) = setup();
        let lane = execution.lane("writer").unwrap();
        let (job, steps, _) = publish(&host, 100);
        lane.push_sliced(job);
        let ran = Arc::new(AtomicBool::new(false));
        let flag = ran.clone();
        let waited = lane.run(
            Box::new(move || flag.store(true, Ordering::SeqCst)),
            Some(Duration::from_millis(10)),
        );
        assert!(!waited, "the limit passed first");
        assert!(!ran.load(Ordering::SeqCst));
        assert_eq!(*steps.lock().unwrap(), [Cooperative::SLICE; 3]);
        // A request without a limit finishes the publish in order, in slices.
        assert!(lane.run(Box::new(|| {}), None));
        assert!(ran.load(Ordering::SeqCst));
        assert_eq!(steps.lock().unwrap().len(), 100);
        assert!(
            steps
                .lock()
                .unwrap()
                .iter()
                .all(|b| *b == Cooperative::SLICE)
        );
    }

    #[test]
    fn closing_cancels_an_unfinished_publish_and_keeps_other_jobs() {
        let (host, execution) = setup();
        let lane = execution.lane("writer").unwrap();
        let (job, steps, cancelled) = publish(&host, 5);
        lane.push_sliced(job);
        let kept = Arc::new(AtomicBool::new(false));
        let flag = kept.clone();
        lane.push(Box::new(move || flag.store(true, Ordering::SeqCst)));
        lane.cancel_sliced();
        assert!(cancelled.load(Ordering::SeqCst));
        assert!(lane.run(Box::new(|| {}), None));
        assert!(kept.load(Ordering::SeqCst));
        assert!(steps.lock().unwrap().is_empty());
    }

    struct Panics;
    impl Sliced for Panics {
        fn step(&mut self, _: Budget) -> Progress {
            panic!("synthetic step failure")
        }
    }

    #[test]
    fn a_panicking_step_or_job_leaves_the_lane_usable() {
        let (_host, execution) = setup();
        let lane = execution.lane("writer").unwrap();
        lane.push_sliced(Box::new(Panics));
        assert!(lane.run(Box::new(|| {}), None));
        let failing = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            lane.run(Box::new(|| panic!("synthetic job failure")), None)
        }));
        assert!(failing.is_err());
        assert!(!lane.is_current(), "the lane is not left running");
        assert!(lane.run(Box::new(|| {}), None));
    }

    #[test]
    fn a_job_cannot_wait_for_its_own_lane_and_there_are_no_workers() {
        let (_host, execution) = setup();
        assert!(execution.shares_connection());
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

    #[test]
    fn tasks_need_not_be_send() {
        let (host, execution) = setup();
        let tasks = execution.tasks();
        let local = std::rc::Rc::new(std::cell::Cell::new(0));
        let shared = local.clone();
        #[cfg(not(feature = "native"))]
        tasks.spawn(Box::pin(async move { shared.set(1) }));
        #[cfg(feature = "native")]
        {
            // Natively tasks are Send; the browser build's are not.
            drop(shared);
            local.set(1);
            tasks.spawn(Box::pin(async {}));
        }
        host.poll();
        assert_eq!(local.get(), 1);
    }
}

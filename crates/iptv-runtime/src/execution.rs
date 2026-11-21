//! The execution port: where the runtime's work runs.
//!
//! A host supplies an [`Execution`] in [`crate::Ports`]. The runtime asks it
//! for what it needs and decides nothing about threads itself:
//!
//! - [`Execution::lane`]: a serial queue of jobs. Each store connection is
//!   one (writer, reader, analysis): jobs run one at a time, in the order
//!   they were queued, off the UI thread.
//! - [`Execution::worker`]: a long-lived worker that runs its body to the
//!   end (the refresh pool, the file writer, the favorites writer).
//! - [`Execution::tasks`]: each owner's task executor for network work,
//!   CPU-bound work, and timers ([`Tasks`]).
//!
//! The native implementation (`native` feature) is threads and a Tokio
//! pool: a thread per lane and per worker, and a two-worker Tokio runtime
//! per owner, exactly as before the port existed. [`Cooperative`] is the
//! browser's: one thread, the owner's, which runs lanes and tasks when the
//! host's event loop polls it. A blocking wait on a lane runs that lane's
//! queue inline instead of waiting for another thread; long publishes run
//! in slices ([`Sliced`]) so the queue behind them, and the page, are not
//! held for the whole publish.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_channel::oneshot;

mod cooperative;
pub use cooperative::{Cooperative, CooperativeHost};

/// A job for a lane or a worker. Jobs cross threads natively.
pub type Job = Box<dyn FnOnce() + Send>;

/// A task for an owner's executor: `Send` when the runtime is threaded.
#[cfg(feature = "native")]
pub type Task = Pin<Box<dyn Future<Output = ()> + Send>>;
/// A task for an owner's executor: `Send` when the runtime is threaded.
#[cfg(not(feature = "native"))]
pub type Task = Pin<Box<dyn Future<Output = ()>>>;

/// A future driven to its end on the calling worker ([`Tasks::block_on`]).
pub type LocalTask<'a> = Pin<Box<dyn Future<Output = ()> + 'a>>;

/// What a host supplies to run the runtime's work.
pub trait Execution: Send + Sync {
    /// A new serial queue named `name` (for diagnostics and thread names).
    fn lane(&self, name: &'static str) -> io::Result<Box<dyn Lane>>;

    /// Starts a long-lived worker named `name` that runs `body` to its end.
    /// Fails when the host has no workers ([`Cooperative`]): the caller
    /// reports the service as unavailable.
    fn worker(&self, name: String, body: Job) -> io::Result<()>;

    /// A task executor for one owner, made on that owner's first use and
    /// dropped with it.
    fn tasks(&self) -> Box<dyn Tasks>;
}

/// A serial queue: jobs run one at a time, in the order they were pushed.
pub trait Lane: Send + Sync {
    /// Queues `job` behind every job queued before it. False when the lane
    /// stopped; the job was dropped without running.
    fn push(&self, job: Job) -> bool;

    /// Queues `job` and returns once it has run: true, or false when the
    /// lane stopped or `limit` passed first (the job still runs later).
    fn run(&self, job: Job, limit: Option<Duration>) -> bool;

    /// Queues a job that runs in slices: [`Sliced::step`] until it reports
    /// [`Progress::Done`]. Natively all steps run at once, in one job; a
    /// cooperative lane runs one step per turn of the host's event loop, so
    /// jobs queued behind wait for the whole of it but the page does not.
    fn push_sliced(&self, mut job: Box<dyn Sliced>) -> bool {
        self.push(Box::new(move || {
            while job.step(Budget::UNLIMITED) == Progress::Pending {}
        }))
    }

    /// The caller is running one of this lane's jobs (a job may not wait for
    /// its own lane).
    fn is_current(&self) -> bool;
}

/// A job that runs in slices, such as a resumable catalog or guide publish.
pub trait Sliced: Send {
    /// Does work worth at most `budget`, then returns.
    fn step(&mut self, budget: Budget) -> Progress;
}

/// How much one [`Sliced::step`] may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// Wall time the step may take; `None` for no limit.
    pub time: Option<Duration>,
}

impl Budget {
    /// No limit: the step runs the job to its end.
    pub const UNLIMITED: Self = Self { time: None };
}

/// What a [`Sliced::step`] left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// More steps are needed.
    Pending,
    /// The job has finished.
    Done,
}

/// One owner's task executor.
pub trait Tasks: Send + Sync {
    /// Runs `task` to its end; nothing waits for it here.
    fn spawn(&self, task: Task);

    /// Runs CPU-bound `work` where it does not hold network tasks.
    fn spawn_blocking(&self, work: Job);

    /// Drives `future` to its end on the calling worker, with the task
    /// executor's timers and I/O available to it. Only workers call this,
    /// never the UI thread.
    fn block_on(&self, future: LocalTask<'_>);

    /// A future that ends after `duration`.
    fn delay(&self, duration: Duration) -> Task;

    /// [`Tasks::block_on`] may wait for I/O and timers. False for a
    /// cooperative executor: work that blocks is refused there, as having
    /// no capacity, instead of being started.
    fn can_block(&self) -> bool {
        true
    }
}

/// The output of spawned work, awaited by whoever spawned it. Dropping it
/// detaches the work (cancel it with its `CancelToken`).
pub struct Join<T>(oneshot::Receiver<T>);

/// The work ended without an output: it panicked, or its executor shut
/// down first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JoinError;

impl std::fmt::Display for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the task ended without an output")
    }
}

impl std::error::Error for JoinError {}

impl<T> Future for Join<T> {
    type Output = Result<T, JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0)
            .poll(cx)
            .map(|result| result.map_err(|_| JoinError))
    }
}

/// Spawns `future` on `tasks`; its output arrives through the [`Join`].
pub(crate) fn spawn<F>(tasks: &dyn Tasks, future: F) -> Join<F::Output>
where
    F: Future + crate::MaybeSend + 'static,
    F::Output: crate::MaybeSend + 'static,
{
    let (sender, receiver) = oneshot::channel();
    tasks.spawn(Box::pin(async move {
        let _ = sender.send(future.await);
    }));
    Join(receiver)
}

/// Runs `work` on `tasks`' blocking side; its output arrives through the
/// [`Join`].
pub(crate) fn spawn_blocking<R: Send + 'static>(
    tasks: &dyn Tasks,
    work: impl FnOnce() -> R + Send + 'static,
) -> Join<R> {
    let (sender, receiver) = oneshot::channel();
    tasks.spawn_blocking(Box::new(move || {
        let _ = sender.send(work());
    }));
    Join(receiver)
}

/// Drives `future` to its end on the calling worker.
pub(crate) fn block_on<F: Future>(tasks: &dyn Tasks, future: F) -> F::Output {
    let mut output = None;
    tasks.block_on(Box::pin(async {
        output = Some(future.await);
    }));
    output.expect("block_on drives its future to the end")
}

/// The deadline passed before the future ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Elapsed;

/// `future`, unless `limit` passes first on `tasks`' timer.
pub(crate) async fn timeout<F: Future>(
    tasks: &dyn Tasks,
    limit: Duration,
    future: F,
) -> Result<F::Output, Elapsed> {
    let mut delay = tasks.delay(limit);
    let mut future = std::pin::pin!(future);
    std::future::poll_fn(move |cx| {
        if let Poll::Ready(output) = future.as_mut().poll(cx) {
            return Poll::Ready(Ok(output));
        }
        delay.as_mut().poll(cx).map(|()| Err(Elapsed))
    })
    .await
}

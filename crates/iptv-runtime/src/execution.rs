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

    /// The store lanes share one connection: a browser store has a single
    /// one (no reader connection), and reads run on the writer's store
    /// between the writer's jobs and slices. Natively each lane has its own.
    fn shares_connection(&self) -> bool {
        false
    }

    /// The host has workers that may block on I/O and timers
    /// ([`Execution::worker`], [`Tasks::block_on`]). False for a cooperative
    /// executor: what needs them (imports, staged probes, the file and
    /// favorites writers) is unavailable there, and reported so, instead of
    /// being started. Asking makes no thread or runtime.
    fn has_workers(&self) -> bool {
        true
    }
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
    /// cooperative lane runs one step per turn of the host's event loop, and
    /// the other lanes' jobs (reads) run between them. Jobs queued behind it
    /// on this lane wait for the whole of it, in order.
    fn push_sliced(&self, job: Box<dyn Sliced>) -> bool {
        self.push(Box::new(move || run_to_end(job)))
    }

    /// Cancels the sliced jobs this lane has not finished
    /// ([`Sliced::cancel`]): the owner is closing, and a publish must be
    /// abandoned, not completed. Natively a sliced job runs as one job to its
    /// end, so there is nothing left to cancel.
    fn cancel_sliced(&self) {}

    /// The caller is running one of this lane's jobs (a job may not wait for
    /// its own lane).
    fn is_current(&self) -> bool;
}

/// Runs `job` to its end in one go; a panicking step ends the job (and is
/// logged), not the lane that runs it.
pub(crate) fn run_to_end(mut job: Box<dyn Sliced>) {
    let stepped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        while job.step(Budget::UNLIMITED) == Progress::Pending {}
    }));
    if stepped.is_err() {
        log::error!("iptv execution: a sliced job panicked; it was dropped");
    }
}

/// A job that runs in slices, such as a resumable catalog or guide publish
/// (`iptv-store-sqlite`'s `CatalogReplacement` and `EpgReplacement`, whose
/// `step(rows)` is this `step`).
///
/// A step that returns [`Progress::Pending`] must have done work: a job
/// that never ends would hold its lane, and a blocking request behind it,
/// forever. A job reports its own failures (an abandoned publish) and then
/// returns [`Progress::Done`].
pub trait Sliced: Send {
    /// Does work worth at most `budget`, then returns.
    fn step(&mut self, budget: Budget) -> Progress;

    /// The owner closed before the job finished: undo what it began (a
    /// publish's `abandon_publish`) instead of finishing it.
    fn cancel(self: Box<Self>) {}
}

/// How much one [`Sliced::step`] may do: rows, as the store's resumable
/// jobs count their work (it needs no clock).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    pub rows: u32,
}

impl Budget {
    /// No limit: the step runs the job to its end.
    pub const UNLIMITED: Self = Self { rows: u32::MAX };
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

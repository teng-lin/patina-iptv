//! The native execution port: a thread per lane and per worker, and per
//! owner one multi-threaded Tokio runtime for network work that is not an
//! import (Stalker link resolves, artwork, on-demand browsing), whose
//! blocking threads also build large schedules off the UI thread.
//!
//! The Tokio runtime has two workers. Its tasks are network waits with
//! short CPU bursts between them (TLS handshakes, a portal's JSON, encoding
//! a 4 MiB logo as a data URI), so a second worker keeps a burst from
//! delaying a zap's link resolve; more workers would only add idle threads
//! on a TV box. A current-thread runtime would need a thread of its own to
//! drive it anyway, and would serialize those bursts. It is made on the
//! owner's first use and shut down when the owner's services are dropped.
//!
//! Tasks hand results to the UI through channels or a `Join`, which wake
//! the NSH task awaiting them; nothing polls. Imports keep their bounded
//! pool (`workers.rs`): their pipelines call the store with blocking
//! requests, which must not hold this runtime's workers.

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::thread::ThreadId;
use std::time::Duration;
use std::{cell::Cell, io};

use tokio::runtime::{Builder, Runtime};

use crate::execution::{Execution, Job, Lane, LocalTask, Task, Tasks};

/// Threads and Tokio, as a native host runs them.
pub(crate) struct NativeExecution;

impl Execution for NativeExecution {
    fn lane(&self, name: &'static str) -> io::Result<Box<dyn Lane>> {
        let (sender, receiver) = mpsc::channel::<Job>();
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                while let Ok(job) = receiver.recv() {
                    job();
                }
            })?;
        Ok(Box::new(ThreadLane {
            sender,
            thread: thread.thread().id(),
        }))
    }

    fn worker(&self, name: String, body: Job) -> io::Result<()> {
        std::thread::Builder::new().name(name).spawn(body).map(drop)
    }

    fn tasks(&self) -> Box<dyn Tasks> {
        Box::new(TokioTasks::default())
    }
}

/// A lane on its own thread, which owns nothing but the queue: its jobs
/// carry what they work on.
pub(crate) struct ThreadLane {
    sender: mpsc::Sender<Job>,
    thread: ThreadId,
}

#[cfg(test)]
impl ThreadLane {
    /// A lane over `sender`, as if its thread were another one.
    pub(crate) fn for_tests(sender: mpsc::Sender<Job>) -> Self {
        let thread = std::thread::spawn(|| std::thread::current().id())
            .join()
            .unwrap();
        Self { sender, thread }
    }
}

impl Lane for ThreadLane {
    fn push(&self, job: Job) -> bool {
        self.sender.send(job).is_ok()
    }

    fn run(&self, job: Job, limit: Option<Duration>) -> bool {
        let (done, reply) = mpsc::sync_channel(1);
        if !self.push(Box::new(move || {
            job();
            let _ = done.send(());
        })) {
            return false;
        }
        match limit {
            Some(limit) => reply.recv_timeout(limit).is_ok(),
            None => reply.recv().is_ok(),
        }
    }

    fn is_current(&self) -> bool {
        self.thread == std::thread::current().id()
    }
}

#[derive(Default)]
struct TokioTasks {
    runtime: OnceLock<Runtime>,
    initializing: Mutex<()>,
}

thread_local! {
    static STARTING_TOKIO_WORKER: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn recovering_worker_spawn(payload: &(dyn std::any::Any + Send)) -> bool {
    STARTING_TOKIO_WORKER.try_with(Cell::get).unwrap_or(false)
        && crate::execution::worker_spawn_refusal(payload).is_some()
}

/// The pinned Tokio runtime panics if its pool cannot spawn a worker, even
/// when that happens inside the fallible runtime builder.
fn try_spawn_worker<T>(start: impl FnOnce() -> T) -> io::Result<T> {
    let previous = STARTING_TOKIO_WORKER.replace(true);
    let result = catch_unwind(AssertUnwindSafe(start));
    STARTING_TOKIO_WORKER.set(previous);
    match result {
        Ok(value) => Ok(value),
        Err(panic) => match crate::execution::worker_spawn_refusal(&*panic) {
            Some(reason) => Err(io::Error::other(reason.to_owned())),
            None => resume_unwind(panic),
        },
    }
}

impl TokioTasks {
    fn try_runtime(&self) -> io::Result<&Runtime> {
        self.try_runtime_with(|| {
            Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("iptv-net")
                .enable_all()
                .build()
        })
    }

    fn try_runtime_with(
        &self,
        build: impl FnOnce() -> io::Result<Runtime>,
    ) -> io::Result<&Runtime> {
        if self.runtime.get().is_none() {
            let _guard = self.initializing.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(runtime) = self.runtime.get() {
                return Ok(runtime);
            }
            let runtime = try_spawn_worker(build)??;
            // Only this initializer can reach `set` while holding the mutex.
            let _ = self.runtime.set(runtime);
        }
        self.runtime
            .get()
            .ok_or_else(|| io::Error::other("network runtime initialization failed"))
    }
}

impl Drop for TokioTasks {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl Tasks for TokioTasks {
    fn ready(&self) -> bool {
        match self.try_runtime() {
            Ok(_) => true,
            Err(error) => {
                log::error!("iptv network runtime could not start: {error}");
                false
            }
        }
    }

    fn spawn(&self, task: Task) {
        match self.try_runtime() {
            Ok(runtime) => drop(runtime.spawn(task)),
            Err(error) => log::error!("iptv network task could not start: {error}"),
        }
    }

    fn spawn_blocking(&self, work: Job) {
        let result = self.try_runtime().and_then(|runtime| {
            // Tokio can leave a task queued when spawning its worker fails.
            // Recover the closure so its caller gets a closed reply channel,
            // and a worker started later will not run that refused operation.
            let work = Arc::new(Mutex::new(Some(work)));
            let queued = work.clone();
            let mut admission = work.lock().unwrap_or_else(|e| e.into_inner());
            let result = try_spawn_worker(|| {
                runtime.spawn_blocking(move || {
                    let job = queued.lock().unwrap_or_else(|e| e.into_inner()).take();
                    if let Some(job) = job {
                        job();
                    }
                })
            });
            let refused = if result.is_err() {
                admission.take()
            } else {
                None
            };
            drop(admission);
            drop(refused);
            result.map(drop)
        });
        if let Err(error) = result {
            log::error!("iptv blocking task could not start: {error}");
        }
    }

    fn block_on(&self, future: LocalTask<'_>) {
        match self.try_runtime() {
            Ok(runtime) => runtime.block_on(future),
            Err(error) => log::error!("iptv network runtime could not drive task: {error}"),
        }
    }

    fn delay(&self, duration: Duration) -> Task {
        // A Tokio timer registers with the runtime it is made in.
        match self.try_runtime() {
            Ok(runtime) => {
                let _context = runtime.enter();
                Box::pin(tokio::time::sleep(duration))
            }
            Err(error) => {
                log::error!("iptv network timer could not start: {error}");
                Box::pin(std::future::ready(()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TokioTasks;

    #[test]
    fn failed_tokio_runtime_initialization_is_returned_without_panicking() {
        let tasks = TokioTasks::default();
        let result = tasks
            .try_runtime_with(|| Err(std::io::Error::other("synthetic runtime spawn refusal")));
        assert!(result.is_err());
        assert!(tasks.runtime.get().is_none());
    }

    #[test]
    fn tokio_worker_spawn_panic_does_not_poison_startup() {
        let tasks = TokioTasks::default();
        let result = tasks.try_runtime_with(|| {
            panic!("OS can't spawn worker thread: synthetic refusal");
        });
        assert!(result.is_err());
        assert!(tasks.runtime.get().is_none());
        assert!(tasks.try_runtime().is_ok());
    }
}

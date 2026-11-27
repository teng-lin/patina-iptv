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

use std::io;
use std::sync::{Mutex, OnceLock, mpsc};
use std::thread::ThreadId;
use std::time::Duration;

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
            let runtime = build()?;
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
        match self.try_runtime() {
            Ok(runtime) => drop(runtime.spawn_blocking(work)),
            Err(error) => log::error!("iptv blocking task could not start: {error}"),
        }
    }

    fn block_on(&self, future: LocalTask<'_>) -> bool {
        match self.try_runtime() {
            Ok(runtime) => {
                runtime.block_on(future);
                true
            }
            Err(error) => {
                log::error!("iptv network runtime could not drive task: {error}");
                false
            }
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
}

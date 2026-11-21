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
use std::sync::{OnceLock, mpsc};
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
}

impl TokioTasks {
    fn runtime(&self) -> &Runtime {
        self.runtime.get_or_init(|| {
            Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("iptv-net")
                .enable_all()
                .build()
                .expect("start the network runtime")
        })
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
    fn spawn(&self, task: Task) {
        drop(self.runtime().spawn(task));
    }

    fn spawn_blocking(&self, work: Job) {
        drop(self.runtime().spawn_blocking(work));
    }

    fn block_on(&self, future: LocalTask<'_>) {
        self.runtime().block_on(future);
    }

    fn delay(&self, duration: Duration) -> Task {
        // A Tokio timer registers with the runtime it is made in.
        let _context = self.runtime().enter();
        Box::pin(tokio::time::sleep(duration))
    }
}

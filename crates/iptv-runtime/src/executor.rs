//! An owner's one runtime for network work that is not an import: Stalker
//! link resolves and artwork downloads; its blocking threads also build
//! large schedules off the UI thread. It is made once, on first use, and
//! is owned by that owner's services (`store_service::Service`), which
//! hand its tasks out through their `Services` handle.
//!
//! Multi-threaded, with two workers. Its tasks are network waits with
//! short CPU bursts between them (TLS handshakes, a portal's JSON,
//! encoding a 4 MiB logo as a data URI), so a second worker keeps a burst
//! from delaying a zap's link resolve; more workers would only add idle
//! threads on a TV box. A current-thread runtime would need a thread of
//! its own to drive it anyway, and would serialise those bursts.
//!
//! Tasks hand results to the UI through channels or the `JoinHandle`,
//! which wake the NSH task awaiting them; nothing polls. Imports keep
//! their bounded pool (`workers.rs`): their pipelines call the store
//! with blocking requests, which must not hold this runtime's workers.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::{Builder, Runtime};
use tokio::task::JoinHandle;

#[derive(Default)]
pub(crate) struct Executor {
    runtime: OnceLock<Runtime>,
}
impl Executor {
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
impl Drop for Executor {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl Executor {
    pub(crate) fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.runtime().spawn(future)
    }

    pub(crate) fn spawn_blocking<R: Send + 'static>(
        &self,
        work: impl FnOnce() -> R + Send + 'static,
    ) -> JoinHandle<R> {
        self.runtime().spawn_blocking(work)
    }

    pub(crate) fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.runtime().block_on(future)
    }
}

//! The app's one runtime for network work that is not an import: Stalker
//! link resolves and artwork downloads; its blocking threads also build
//! large schedules off the UI thread. It is made once, on first use, and
//! is owned by the application services.
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

/// Runs `future` on the runtime; await the handle for its output. Dropping
/// the handle detaches the task (cancel it with its `CancelToken`).
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let owner = super::store_service::service();
    let future = super::store_service::scope_future(owner.clone(), future);
    owner.executor.runtime().spawn(future)
}

/// Runs CPU-bound `work` on the runtime's blocking threads (building a
/// large schedule); await the handle for its output.
pub fn spawn_blocking<R: Send + 'static>(
    work: impl FnOnce() -> R + Send + 'static,
) -> JoinHandle<R> {
    let owner = super::store_service::service();
    owner
        .executor
        .runtime()
        .spawn_blocking(super::store_service::scoped(work))
}

/// Drive a non-Send import on its bounded blocking worker, sharing the IO executor.
pub fn block_on<F: Future>(future: F) -> F::Output {
    super::store_service::service()
        .executor
        .runtime()
        .block_on(future)
}

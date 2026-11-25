//! Catalog and guide publishes where the store is the host's
//! ([`crate::store_host::StoreHost`], a browser): a sliced job on the
//! writer lane, so a publish of seconds never holds the app's one thread.
//!
//! Natively a publish is one writer job, whose transaction the reader
//! connection does not wait for (WAL). This is the same contract on one
//! connection: the replacement runs on a publish copy, at most one step
//! of the executor's slice per turn ([`crate::execution::Cooperative`]);
//! reads between the steps see the live store's last commit; writes queued
//! behind it on the writer lane wait for the swap, in order, so none is
//! lost (the live store grants no write lock while the copy exists).
//!
//! A publish ([`Publish`]) has three parts, each on the writer lane:
//!
//! 1. [`Publish::prepare`] on the live store: the checks that admit it and
//!    the writes that come first (a new source's row), as the one-shot
//!    form makes them in the same job.
//! 2. [`Publish::start`] on the copy, then [`Publish::step`] until the
//!    replacement committed its last step.
//! 3. The swap, then [`Publish::finish`] on the live store: the writes
//!    that follow (feed results, pruning), and the reply.
//!
//! Before the swap, an error, a closed owner, a shutdown or a publish
//! cancellation ([`Publish::canceled`], checked before every step and the
//! swap) abandons the copy. [`Publish::failed`] then undoes what
//! `prepare` wrote. After the swap, an error in [`Publish::finish`]
//! leaves the replacement live.

use std::future::Future;
use std::sync::{Arc, Mutex};

use futures_channel::oneshot;
use iptv_store_sqlite::{Step, Store, StoreError};

use crate::execution::{Budget, Progress, Sliced};
use crate::store_host::StoreHost;
use crate::store_service::{Connection, Service, Services};

/// One catalog or guide replacement, run in slices on a publish copy.
pub(crate) trait Publish: Send + 'static {
    type Output: Send + 'static;

    /// On the live store, before the copy is made.
    fn prepare(&mut self, live: &mut Store) -> Result<(), StoreError>;

    /// Starts the replacement on `copy`, which it owns from here.
    fn start(&mut self, copy: Store) -> Result<(), StoreError>;

    /// Writes about `rows`; true once the replacement committed its last
    /// step.
    fn step(&mut self, rows: u32) -> Result<bool, StoreError>;

    /// The copy, back from the replacement (finished, or to abandon).
    fn take_copy(&mut self) -> Option<Store>;

    /// On the live store, after the swap.
    fn finish(self: Box<Self>, live: &mut Store) -> Result<Self::Output, StoreError>;

    /// The publish ended without a swap: undo what `prepare` wrote.
    fn failed(&mut self, _live: &mut Store) {}

    /// Whether the publish lost its right to land since `prepare` admitted
    /// it (the user canceled the import, newer work superseded the guide).
    /// The one-shot form checks once, in the same job as its writes; here
    /// slices of other turns follow `prepare`, so the driver checks before
    /// each step and before the swap.
    fn canceled(&self) -> bool {
        false
    }
}

/// What a canceled publish replies (the one-shot forms' refusal).
const CANCELED: StoreError = StoreError::Unavailable("the publish was canceled");

/// A replacement job as [`Publish::step`] drives it: the store's
/// `CatalogReplacement` and `EpgReplacement`.
pub(crate) trait Replacement: Send {
    type Done: Send;
    fn step(&mut self, rows: u32) -> Result<Step<Self::Done>, StoreError>;
    fn into_store(self) -> Store;
}

impl Replacement for iptv_store_sqlite::CatalogReplacement<'static, Store> {
    type Done = iptv_store_sqlite::CatalogPublished;
    fn step(&mut self, rows: u32) -> Result<Step<Self::Done>, StoreError> {
        iptv_store_sqlite::CatalogReplacement::step(self, rows)
    }
    fn into_store(self) -> Store {
        iptv_store_sqlite::CatalogReplacement::into_store(self)
    }
}

impl Replacement for iptv_store_sqlite::EpgReplacement<'static, Store> {
    type Done = iptv_store_sqlite::EpgCounts;
    fn step(&mut self, rows: u32) -> Result<Step<Self::Done>, StoreError> {
        iptv_store_sqlite::EpgReplacement::step(self, rows)
    }
    fn into_store(self) -> Store {
        iptv_store_sqlite::EpgReplacement::into_store(self)
    }
}

/// A replacement in progress and, once it is done, what it reported.
pub(crate) enum Replacing<R: Replacement> {
    Idle,
    Stepping(R),
    /// The job committed: its copy until it is taken, and its report.
    Done(Option<Store>, R::Done),
}

impl<R: Replacement> Replacing<R> {
    /// One step; true when the job committed its last one.
    pub(crate) fn step(&mut self, rows: u32) -> Result<bool, StoreError> {
        let Self::Stepping(job) = self else {
            return Ok(matches!(self, Self::Done(..)));
        };
        match job.step(rows)? {
            Step::Pending => Ok(false),
            Step::Done(done) => {
                let Self::Stepping(job) = std::mem::replace(self, Self::Idle) else {
                    unreachable!("matched above");
                };
                *self = Self::Done(Some(job.into_store()), done);
                Ok(true)
            }
        }
    }

    /// The copy back: from a finished job (its report stays), or from an
    /// unfinished one, which rolls back.
    pub(crate) fn take_copy(&mut self) -> Option<Store> {
        match self {
            Self::Idle => None,
            Self::Stepping(_) => match std::mem::replace(self, Self::Idle) {
                Self::Stepping(job) => Some(job.into_store()),
                _ => unreachable!("matched above"),
            },
            Self::Done(store, _) => store.take(),
        }
    }

    /// What the finished job reported.
    pub(crate) fn done(self) -> Option<R::Done> {
        match self {
            Self::Done(_, done) => Some(done),
            _ => None,
        }
    }
}

impl Services {
    /// Runs `publish` as a sliced job on the writer lane and returns its
    /// reply. For a host that keeps the store ([`crate::Ports::store_host`]);
    /// natively a publish is one writer job (the callers' one-shot form).
    pub(crate) fn publish<P: Publish>(
        &self,
        publish: P,
    ) -> impl Future<Output = Result<P::Output, StoreError>> + use<P> {
        let (reply, answer) = oneshot::channel();
        let queued = match self.ports().store_host.clone() {
            Some(host) if self.is_current() => {
                let job = Job {
                    service: self.service().clone(),
                    host,
                    epoch: self.epoch(),
                    connection: self.service().writer_connection(),
                    publish: Some(Box::new(publish)),
                    phase: Phase::Prepare,
                    reply: Some(reply),
                    timing: Timing::default(),
                };
                self.service().push_sliced(Box::new(job))
            }
            Some(_) => Err(StoreError::Unavailable(
                "request belongs to a closed application",
            )),
            None => Err(StoreError::Unavailable(
                "a sliced publish needs a store host",
            )),
        };
        async move {
            queued?;
            answer
                .await
                .map_err(|_| StoreError::Unavailable("store reply lost"))?
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// `prepare` and the copy come next.
    Prepare,
    /// The copy exists: steps until the replacement is done.
    Steps,
}

struct Job<P: Publish> {
    service: Arc<Service>,
    host: Arc<dyn StoreHost>,
    epoch: u64,
    connection: Arc<Mutex<Connection>>,
    publish: Option<Box<P>>,
    phase: Phase,
    reply: Option<oneshot::Sender<Result<P::Output, StoreError>>>,
    /// Steps run, their total and the longest (the plan's 50 ms gate for
    /// one step on a TV), logged when the publish ends.
    timing: Timing,
}

#[derive(Default)]
struct Timing {
    steps: u32,
    total: std::time::Duration,
    longest: std::time::Duration,
}

impl Timing {
    fn log(&self, outcome: &str) {
        log::info!(
            "iptv store: publish {outcome}: {} steps, {} ms in steps, longest {} ms",
            self.steps,
            self.total.as_millis(),
            self.longest.as_millis()
        );
    }
}

impl<P: Publish> Job<P> {
    /// Abandons the copy, if there is one, undoes `prepare`'s writes, and
    /// replies `error`.
    fn fail(&mut self, live: Option<&mut Store>, error: StoreError) {
        if let Some(publish) = self.publish.as_mut() {
            let copy = publish.take_copy();
            if let Some(live) = live {
                if let Some(copy) = copy {
                    self.host.abandon_publish(live, copy);
                }
                if self.phase == Phase::Steps {
                    publish.failed(live);
                }
            }
        }
        self.publish = None;
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(Err(error));
        }
    }

    /// One step on the live store `live`: true when the job has ended.
    fn advance(&mut self, live: &mut Store, rows: u32) -> Result<bool, StoreError> {
        let publish = self.publish.as_mut().expect("a running publish");
        match self.phase {
            Phase::Prepare => {
                publish.prepare(live)?;
                self.phase = Phase::Steps;
                let copy = self.host.begin_publish(live)?;
                publish.start(copy)?;
                Ok(false)
            }
            Phase::Steps => {
                if publish.canceled() {
                    return Err(CANCELED);
                }
                if !publish.step(rows)? {
                    return Ok(false);
                }
                // A cancel during the last step: the copy is complete but
                // must not replace the live store.
                if publish.canceled() {
                    return Err(CANCELED);
                }
                let copy = publish
                    .take_copy()
                    .ok_or(StoreError::Unavailable("the publish copy is gone"))?;
                self.host.finish_publish(live, copy)?;
                let publish = self.publish.take().expect("a running publish");
                let output = publish.finish(live);
                // The swap and what followed it, saved as one commit's.
                self.service.persist(live);
                if let Some(reply) = self.reply.take() {
                    let _ = reply.send(output);
                }
                Ok(true)
            }
        }
    }
}

impl<P: Publish> Sliced for Job<P> {
    fn step(&mut self, budget: Budget) -> Progress {
        if self.publish.is_none() {
            return Progress::Done;
        }
        let connection = self.connection.clone();
        let mut connection = connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let service = self.service.clone();
        let started = crate::time::Instant::now();
        let mut failed = false;
        let ended = service.with_writer_store(self.epoch, &mut connection, |live| {
            match self.advance(live, budget.rows) {
                Ok(ended) => Ok(ended),
                Err(error) => {
                    failed = true;
                    self.fail(Some(live), error);
                    Ok(true)
                }
            }
        });
        let took = started.elapsed();
        self.timing.steps += 1;
        self.timing.total += took;
        self.timing.longest = self.timing.longest.max(took);
        match ended {
            Ok(false) => Progress::Pending,
            Ok(true) => {
                self.timing
                    .log(if failed { "abandoned" } else { "finished" });
                Progress::Done
            }
            // The owner closed, or the store would not open: nothing to
            // swap into. Dropping the copy abandons the publish.
            Err(error) => {
                self.fail(None, error);
                Progress::Done
            }
        }
    }

    fn cancel(mut self: Box<Self>) {
        let connection = self.connection.clone();
        let mut connection = connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let service = self.service.clone();
        let closed = || StoreError::Unavailable("the application closed during a publish");
        let abandoned = service.with_writer_store(self.epoch, &mut connection, |live| {
            self.fail(Some(live), closed());
            Ok(())
        });
        if abandoned.is_err() {
            self.fail(None, closed());
        }
    }
}

#[cfg(all(test, feature = "native"))]
mod tests {
    //! The driver on a native store: the fake host's "copy" is a second
    //! connection to the same file, whose transaction spans the steps, so
    //! the live connection reads the last commit meanwhile (WAL), as the
    //! browser's copy lets it.
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;

    use iptv_providers::model::{Channel, Group, M3uSourceConfig, Source, SourceConfig};
    use iptv_store_sqlite::{
        CatalogReplacement, ChannelQuery, DatabaseKey, SourceId, Store, StoreError,
    };

    use super::{Publish, Replacing};
    use crate::execution::{Cooperative, CooperativeHost, Task};
    use crate::store_host::{Persist, StoreHost};
    use crate::store_service::Service;

    /// A synthetic event loop: spawned tasks wait for `poll`, which runs
    /// rounds while one was woken.
    #[derive(Default)]
    struct Loop {
        tasks: Mutex<Vec<Task>>,
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
            self.tasks.lock().unwrap().push(task);
            self.woken.store(true, Ordering::SeqCst);
        }
        fn delay(&self, _duration: Duration) -> Task {
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
            Duration::ZERO
        }
    }

    impl Loop {
        fn poll_round(&self, context: &mut Context<'_>) {
            self.woken.store(false, Ordering::SeqCst);
            let mut tasks = std::mem::take(&mut *self.tasks.lock().unwrap());
            tasks.retain_mut(|task| task.as_mut().poll(context).is_pending());
            self.tasks.lock().unwrap().extend(tasks);
        }

        /// One delivery; false when no task is left.
        fn poll(&self) -> bool {
            let waker = Waker::from(Arc::new(Woken(self.woken.clone())));
            let mut context = Context::from_waker(&waker);
            loop {
                self.poll_round(&mut context);
                if !self.woken.load(Ordering::SeqCst) {
                    break;
                }
            }
            !self.tasks.lock().unwrap().is_empty()
        }
    }

    const KEY: [u8; 32] = [0x5c; 32];
    const PLAYLIST_URL: &str = "https://example.invalid/publish.m3u";

    #[derive(Default)]
    struct Calls {
        persisted: AtomicU32,
        begun: AtomicU32,
        finished: AtomicU32,
        abandoned: AtomicU32,
    }

    struct FileHost {
        path: std::path::PathBuf,
        calls: Arc<Calls>,
    }

    impl StoreHost for FileHost {
        fn open(&self) -> Result<Store, StoreError> {
            Store::open_encrypted(&self.path, &DatabaseKey::from_bytes(KEY))
        }
        fn committed(&self, _store: &Store, _again: Persist) {
            self.calls.persisted.fetch_add(1, Ordering::SeqCst);
        }
        fn begin_publish(&self, _store: &mut Store) -> Result<Store, StoreError> {
            self.calls.begun.fetch_add(1, Ordering::SeqCst);
            self.open()
        }
        fn finish_publish(&self, _store: &mut Store, copy: Store) -> Result<(), StoreError> {
            self.calls.finished.fetch_add(1, Ordering::SeqCst);
            drop(copy);
            Ok(())
        }
        fn abandon_publish(&self, _store: &mut Store, copy: Store) {
            self.calls.abandoned.fetch_add(1, Ordering::SeqCst);
            drop(copy);
        }
    }

    fn synthetic_source() -> Source {
        Source {
            name: "Synthetic publish".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: iptv_providers::SecretUrl::new(PLAYLIST_URL),
                user_agent: None,
                referrer: None,
            }),
        }
    }

    fn catalog(channels: usize) -> (Vec<Group>, Vec<Channel>) {
        let mut text = String::from("#EXTM3U\n");
        for n in 0..channels {
            text.push_str(&format!(
                "#EXTINF:-1 tvg-id=\"syn{n}\" group-title=\"Synthetic\",Synthetic {n}\nhttps://example.invalid/{n}.ts\n"
            ));
        }
        let playlist = iptv_providers::m3u::parse_m3u(&text);
        (playlist.groups, playlist.channels.items)
    }

    /// A new source and its catalog, published in slices.
    struct Fixture {
        catalog: Option<(Vec<Group>, Vec<Channel>)>,
        source: Option<SourceId>,
        steps: Arc<AtomicU32>,
        running: Replacing<CatalogReplacement<'static, Store>>,
        /// The publish's cancel ([`Publish::canceled`]).
        cancel: Arc<AtomicBool>,
        /// Cancels in the step that commits the replacement's last one.
        cancel_when_done: bool,
    }

    impl Publish for Fixture {
        type Output = (SourceId, u64);
        fn prepare(&mut self, live: &mut Store) -> Result<(), StoreError> {
            self.source = Some(live.add_source(&synthetic_source())?);
            Ok(())
        }
        fn start(&mut self, copy: Store) -> Result<(), StoreError> {
            let (groups, channels) = self.catalog.take().unwrap();
            self.running = Replacing::Stepping(CatalogReplacement::new(
                copy,
                self.source.unwrap(),
                groups,
                channels,
                Vec::new(),
                Vec::new(),
            ));
            Ok(())
        }
        fn step(&mut self, rows: u32) -> Result<bool, StoreError> {
            self.steps.fetch_add(1, Ordering::SeqCst);
            let done = self.running.step(rows)?;
            if done && self.cancel_when_done {
                self.cancel.store(true, Ordering::SeqCst);
            }
            Ok(done)
        }
        fn canceled(&self) -> bool {
            self.cancel.load(Ordering::SeqCst)
        }
        fn take_copy(&mut self) -> Option<Store> {
            self.running.take_copy()
        }
        fn finish(self: Box<Self>, live: &mut Store) -> Result<(SourceId, u64), StoreError> {
            let id = self.source.unwrap();
            Ok((id, live.channel_count(id, &ChannelQuery::default())?))
        }
        fn failed(&mut self, live: &mut Store) {
            if let Some(id) = self.source {
                let _ = live.delete_source(id);
            }
        }
    }

    fn setup() -> (tempfile::TempDir, Arc<Loop>, Arc<Service>, Arc<Calls>) {
        setup_with_network(Arc::new(crate::native::network::NativeNetwork))
    }

    fn setup_with_network(
        network: Arc<dyn crate::network::Network>,
    ) -> (tempfile::TempDir, Arc<Loop>, Arc<Service>, Arc<Calls>) {
        let directory = tempfile::tempdir().unwrap();
        let host = Arc::new(Loop::default());
        let calls = Arc::new(Calls::default());
        let mut ports = crate::Ports::with_adapters(
            directory.path().to_owned(),
            |_| Ok(DatabaseKey::from_bytes(KEY)),
            network,
            Arc::new(Cooperative::new(host.clone())),
        );
        ports.store_host = Some(Arc::new(FileHost {
            path: directory.path().join("synthetic.sqlite"),
            calls: calls.clone(),
        }));
        (directory, host, Service::started_for_tests(ports), calls)
    }

    fn fixture(channels: usize, steps: &Arc<AtomicU32>) -> Fixture {
        Fixture {
            catalog: Some(catalog(channels)),
            source: None,
            steps: steps.clone(),
            running: Replacing::Idle,
            cancel: Arc::new(AtomicBool::new(false)),
            cancel_when_done: false,
        }
    }

    /// The store the publish wrote to, opened again.
    fn reopen(directory: &tempfile::TempDir) -> Store {
        Store::open_encrypted(
            directory.path().join("synthetic.sqlite"),
            &DatabaseKey::from_bytes(KEY),
        )
        .unwrap()
    }

    #[test]
    fn a_publish_runs_in_slices_with_reads_between_and_writes_after() {
        let (_directory, host, service, calls) = setup();
        let services = service.accept();
        let steps = Arc::new(AtomicU32::new(0));
        let log = Arc::new(Mutex::new(Vec::new()));
        let published = Arc::new(Mutex::new(None));
        {
            let (services, log, published, steps) = (
                services.clone(),
                log.clone(),
                published.clone(),
                steps.clone(),
            );
            host.spawn(Box::pin(async move {
                let publish = services.publish(fixture(1_000, &steps));
                // A read queued after the publish runs between its steps,
                // on the last commit; a write waits for the swap.
                let counted = steps.clone();
                let read = services.read(move |store| {
                    Ok((counted.load(Ordering::SeqCst), store.sources()?.len()))
                });
                let order = log.clone();
                let write = services.call(move |store| {
                    order.lock().unwrap().push("write");
                    store.set_combined_last_channel("synthetic-after")
                });
                let read = read.await.unwrap();
                log.lock().unwrap().push("read");
                *published.lock().unwrap() = Some((read, publish.await));
                write.await.unwrap();
            }));
        }
        let mut polls = 0;
        while host.poll() {
            polls += 1;
            assert!(polls < 1_000, "the publish ends");
        }
        let ((steps_at_read, sources_at_read), result) = published.lock().unwrap().take().unwrap();
        let (_, channels) = result.unwrap();
        assert_eq!(channels, 1_000);
        let total = steps.load(Ordering::SeqCst);
        assert!(total >= 5, "200-row slices: {total} steps");
        assert!(steps_at_read < total, "the read ran during the publish");
        assert_eq!(sources_at_read, 1, "it saw the committed source row");
        assert_eq!(*log.lock().unwrap(), ["read", "write"]);
        assert_eq!(calls.begun.load(Ordering::SeqCst), 1);
        assert_eq!(calls.finished.load(Ordering::SeqCst), 1);
        assert_eq!(calls.abandoned.load(Ordering::SeqCst), 0);
        assert!(
            calls.persisted.load(Ordering::SeqCst) >= 2,
            "after jobs and the swap"
        );
        assert!(service.drain_on_shutdown_with(|| {}));
    }

    #[test]
    fn closing_abandons_a_publish_and_undoes_what_it_prepared() {
        let (_directory, host, service, calls) = setup();
        let services = service.accept();
        let steps = Arc::new(AtomicU32::new(0));
        let answer = Arc::new(Mutex::new(None));
        {
            let (services, answer) = (services.clone(), answer.clone());
            let steps = steps.clone();
            host.spawn(Box::pin(async move {
                let result = services.publish(fixture(2_000, &steps)).await;
                *answer.lock().unwrap() = Some(result.is_err());
            }));
        }
        // Prepared and begun, a few slices in.
        for _ in 0..3 {
            host.poll();
        }
        assert_eq!(calls.begun.load(Ordering::SeqCst), 1);
        assert!(service.drain_on_shutdown_with(|| {}));
        while host.poll() {}
        assert_eq!(*answer.lock().unwrap(), Some(true));
        assert_eq!(calls.abandoned.load(Ordering::SeqCst), 1);
        assert_eq!(calls.finished.load(Ordering::SeqCst), 0);
        let store = Store::open_encrypted(
            _directory.path().join("synthetic.sqlite"),
            &DatabaseKey::from_bytes(KEY),
        )
        .unwrap();
        assert!(
            store.sources().unwrap().is_empty(),
            "the prepared source is gone"
        );
    }

    #[test]
    fn a_cancel_between_slices_abandons_the_copy_and_undoes_what_it_prepared() {
        let (directory, host, service, calls) = setup();
        let services = service.accept();
        let steps = Arc::new(AtomicU32::new(0));
        let publish = fixture(2_000, &steps);
        let cancel = publish.cancel.clone();
        let answer = Arc::new(Mutex::new(None));
        {
            let answer = answer.clone();
            host.spawn(Box::pin(async move {
                let result = services.publish(publish).await;
                *answer.lock().unwrap() = Some(result.map(|(_, channels)| channels));
            }));
        }
        // Prepared and begun, a few slices in.
        for _ in 0..3 {
            host.poll();
        }
        assert_eq!(calls.begun.load(Ordering::SeqCst), 1);
        let before = steps.load(Ordering::SeqCst);
        cancel.store(true, Ordering::SeqCst);
        while host.poll() {}
        assert!(
            matches!(
                *answer.lock().unwrap(),
                Some(Err(StoreError::Unavailable(_)))
            ),
            "the publish reports its cancel"
        );
        assert_eq!(
            steps.load(Ordering::SeqCst),
            before,
            "no slice ran after the cancel"
        );
        assert_eq!(calls.abandoned.load(Ordering::SeqCst), 1);
        assert_eq!(calls.finished.load(Ordering::SeqCst), 0, "no swap");
        assert!(
            reopen(&directory).sources().unwrap().is_empty(),
            "the prepared source is gone"
        );
        assert!(service.drain_on_shutdown_with(|| {}));
    }

    #[test]
    fn a_cancel_in_the_last_slice_keeps_the_finished_copy_out() {
        let (directory, host, service, calls) = setup();
        let services = service.accept();
        let steps = Arc::new(AtomicU32::new(0));
        let mut publish = fixture(500, &steps);
        publish.cancel_when_done = true;
        let answer = Arc::new(Mutex::new(None));
        {
            let answer = answer.clone();
            host.spawn(Box::pin(async move {
                let result = services.publish(publish).await;
                *answer.lock().unwrap() = Some(result.map(|(_, channels)| channels));
            }));
        }
        while host.poll() {}
        assert!(
            matches!(
                *answer.lock().unwrap(),
                Some(Err(StoreError::Unavailable(_)))
            ),
            "the publish reports its cancel"
        );
        assert_eq!(calls.begun.load(Ordering::SeqCst), 1);
        assert_eq!(calls.abandoned.load(Ordering::SeqCst), 1);
        assert_eq!(calls.finished.load(Ordering::SeqCst), 0, "no swap");
        assert!(
            reopen(&directory).sources().unwrap().is_empty(),
            "the prepared source is gone"
        );
        assert!(service.drain_on_shutdown_with(|| {}));
    }

    mod catalog_import_tests;
}

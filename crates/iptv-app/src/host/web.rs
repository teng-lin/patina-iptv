//! The browser TV host (Samsung Tizen first): the whole app in one Web
//! Worker, which the page drives (keys in, drawing and the player out).
//!
//! The runtime's ports use `iptv-web`'s adapters for browser fetch,
//! IndexedDB, and cooperative execution on the Worker's event loop.
//!
//! - The store is the browser store (`store.rs`): SQLite on memory files,
//!   restored from IndexedDB at launch and saved in chunks after each
//!   commit, without a key (no key store: `docs/security.md`). Catalog
//!   and guide publishes run in slices on a copy.
//! - Imports and guide downloads run as tasks, one at a time, on the
//!   cooperative executor ([`IMPORTS`]).
//! - Randomness and the local time zone come from the host's imports.
//! - Favorites, recent channels and the other lists are the store's,
//!   saved with it; settings and each playlist's last channel are small
//!   files kept in IndexedDB beside it ([`PageFiles`]).
//! - Recording, backup files and staged probes are unavailable (the
//!   runtime's capability model reports them so).
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use iptv_import::{BodyStream, Fetch, FetchError, FetchErrorKind, StreamingResponse};
use iptv_providers::{HttpRequest, HttpResponse};
use iptv_web::adapters::{HostBody, HostFetch};

use crate::runtime::execution::{Cooperative, CooperativeHost, Task};

mod store;
use crate::runtime::network::{Body, Chunks, Client, NetFuture, Network, NetworkConfig, Transport};

/// A browser TV is none of the native platforms: one decoder, no
/// background work, no OS registration (`iptv_core::capabilities::Host`).
pub(crate) const PLATFORM: iptv_core::capabilities::Platform =
    iptv_core::capabilities::Platform::Unknown;

/// The cooperative executor has no blocking workers
/// (`Runtime::has_workers`): no imports, recording or backups yet.
pub(crate) const HAS_WORKERS: bool = false;

/// A browser TV is driven by its remote, whose keys arrive with their
/// press and release (`input.rs`); its platform has no keyboard bindings
/// (NSH's keymap mount item is not shown there).
pub(crate) const REMOTE_ONLY: bool = true;

/// Imports and guide downloads run here as cooperative tasks over the
/// browser store's sliced publishes.
pub(crate) const IMPORTS: bool = true;

/// The name the store's memory files are kept under.
const DIRECTORY: &str = "/patina";

/// The runtime's ports on a browser TV. The runtime reads the host's
/// clocks too: the Worker has no other.
pub(crate) fn ports(directory: PathBuf) -> crate::runtime::Ports {
    crate::runtime::time::install(crate::runtime::time::Clock {
        monotonic: iptv_web::adapters::monotonic,
        wall: iptv_web::adapters::wall,
    });
    let mut ports = crate::runtime::Ports::with_adapters(
        directory,
        |_| {
            Err(crate::runtime::model::StoreError::unavailable(
                "this device has no key store",
            ))
        },
        Arc::new(PageNetwork),
        Arc::new(Cooperative::new(Arc::new(WorkerLoop))),
    );
    ports.startup_mark = |phase, detail| crate::startup::mark_with(phase, detail);
    ports.file_storage = false;
    ports.entropy = iptv_web::adapters::fill_random;
    ports.store_host = Some(Arc::new(store::BrowserStore::default()));
    // One set for the process: the host hands its stored files over once,
    // and every owner after the first sees the writes of those before it.
    use crate::runtime::file_host::FileHost;
    static FILES: std::sync::OnceLock<Option<Arc<dyn FileHost>>> = std::sync::OnceLock::new();
    ports.file_host = FILES
        .get_or_init(|| {
            let files = iptv_web::adapters::files::StoredFiles::restore()?;
            Some(Arc::new(PageFiles(files)))
        })
        .clone();
    ports
}

/// The app's small files in the page's IndexedDB, by their path under
/// [`DIRECTORY`] (`iptv_web::adapters::files`).
struct PageFiles(iptv_web::adapters::files::StoredFiles);

impl crate::runtime::file_host::FileHost for PageFiles {
    fn read(&self, path: &std::path::Path) -> Option<Vec<u8>> {
        self.0.read(&path.to_string_lossy())
    }

    fn write(
        &self,
        path: &std::path::Path,
        content: Option<Vec<u8>>,
    ) -> crate::runtime::file_host::Stored {
        Box::pin(self.0.write(&path.to_string_lossy(), content))
    }
}

/// The page's fetch, through the Worker's host.
struct PageNetwork;

impl Network for PageNetwork {
    /// The page's fetch cannot cap redirects, the connection time or the
    /// `User-Agent` (the browser owns them); the idle timeout goes to the
    /// host; the client's ceiling and byte budgets are enforced here and in
    /// the client.
    fn client(&self, config: &NetworkConfig) -> Result<Client, FetchError> {
        let fetch = HostFetch::new().with_idle_timeout(config.read_timeout);
        Ok(Client::new(
            Page {
                fetch,
                buffered: config.max_buffered_bytes,
            },
            config.clone(),
        ))
    }
}

struct Page {
    fetch: HostFetch,
    /// A buffered response is read as a stream up to this many bytes, so
    /// the Worker never holds a larger one whole.
    buffered: usize,
}

/// The longest wait a host timer holds (`setTimeout` counts milliseconds
/// in a signed 32-bit integer).
const LONGEST_TIMER: Duration = Duration::from_millis(i32::MAX as u64);

/// When a request's total time ends, with one host timer for all of it:
/// the response and every chunk of its body race the same timer.
struct Deadline {
    /// On the request's `HostFetch` clock.
    at: Duration,
    timer: Pin<Box<dyn Future<Output = ()>>>,
}

impl Deadline {
    fn new(fetch: &HostFetch, request: &HttpRequest) -> Self {
        // The client always sets a timeout; a request without one waits as
        // long as a host timer can.
        let timeout = request.timeout.unwrap_or(LONGEST_TIMER).min(LONGEST_TIMER);
        let timer = fetch.clone();
        Self {
            at: fetch.elapsed() + timeout,
            timer: Box::pin(async move { timer.sleep(timeout).await }),
        }
    }

    /// `work`, unless the deadline passes first. The clock is checked before
    /// `work` is polled, so a result that arrives late is not taken.
    async fn within<T>(
        &mut self,
        fetch: &HostFetch,
        work: impl Future<Output = Result<T, FetchError>>,
    ) -> Result<T, FetchError> {
        let mut work = std::pin::pin!(work);
        let expired = || FetchError::new(FetchErrorKind::Deadline, "total request time exceeded");
        std::future::poll_fn(move |cx| {
            if fetch.elapsed() >= self.at {
                return Poll::Ready(Err(expired()));
            }
            if let Poll::Ready(result) = work.as_mut().poll(cx) {
                return Poll::Ready(result);
            }
            self.timer.as_mut().poll(cx).map(|()| Err(expired()))
        })
        .await
    }
}

impl Transport for Page {
    fn elapsed(&self) -> Duration {
        self.fetch.elapsed()
    }

    fn sleep(&self, duration: Duration) -> NetFuture<'_, ()> {
        Box::pin(self.fetch.sleep(duration))
    }

    fn fetch(&self, request: HttpRequest) -> NetFuture<'_, Result<HttpResponse, FetchError>> {
        let mut deadline = Deadline::new(&self.fetch, &request);
        Box::pin(async move {
            let mut response = deadline
                .within(&self.fetch, self.fetch.fetch_stream(request))
                .await?;
            if response.is_success()
                && response
                    .content_length
                    .is_some_and(|length| length > self.buffered as u64)
            {
                return Err(FetchError::new(
                    FetchErrorKind::TooLarge,
                    "response exceeds its byte budget",
                ));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = deadline
                .within(&self.fetch, response.body.next_chunk())
                .await?
            {
                if bytes.len() + chunk.len() > self.buffered {
                    return Err(FetchError::new(
                        FetchErrorKind::TooLarge,
                        "response exceeds its byte budget",
                    ));
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(HttpResponse::new(response.status, bytes))
        })
    }

    fn fetch_stream(
        &self,
        request: HttpRequest,
    ) -> NetFuture<'_, Result<StreamingResponse<Body>, FetchError>> {
        let mut deadline = Deadline::new(&self.fetch, &request);
        Box::pin(async move {
            let response = deadline
                .within(&self.fetch, self.fetch.fetch_stream(request))
                .await?;
            Ok(StreamingResponse {
                status: response.status,
                content_length: response.content_length,
                body: Body::new(PageBody {
                    body: Some(response.body),
                    fetch: self.fetch.clone(),
                    deadline,
                }),
            })
        })
    }
}

/// A streamed body whose chunks end with the request's total time.
struct PageBody {
    /// Dropped (which closes it in the host) when the deadline passes.
    body: Option<HostBody>,
    fetch: HostFetch,
    deadline: Deadline,
}

impl Chunks for PageBody {
    fn next_chunk(&mut self) -> NetFuture<'_, Result<Option<Vec<u8>>, FetchError>> {
        Box::pin(async move {
            let Some(body) = self.body.as_mut() else {
                return Err(FetchError::new(
                    FetchErrorKind::Deadline,
                    "total request time exceeded",
                ));
            };
            let chunk = self.deadline.within(&self.fetch, body.next_chunk()).await;
            if chunk.is_err() {
                self.body = None;
            }
            chunk
        })
    }
}

/// The Worker's event loop: `iptv-web`'s executor, polled after every
/// delivery from the host. Its tasks have real wakers: a lane turn woken
/// by work queued from an NSH task asks the host for a poll. The NSH TV
/// platform also calls `iptv_web::adapters::poll` after each of its own
/// deliveries (step 2).
struct WorkerLoop;

impl CooperativeHost for WorkerLoop {
    fn spawn(&self, task: Task) {
        iptv_web::adapters::spawn(task);
    }

    fn delay(&self, duration: Duration) -> Task {
        Box::pin(iptv_web::adapters::sleep(duration))
    }

    fn now(&self) -> Duration {
        iptv_web::adapters::monotonic()
    }
}

/// Private storage: the store's memory files, under one name.
pub(crate) fn resolve_data_dir() -> std::io::Result<PathBuf> {
    Ok(PathBuf::from(DIRECTORY))
}

/// As [`resolve_data_dir`]: there is no directory to create or protect.
/// A package build (feature `browser-package`) reads the interface
/// languages' messages from the package from here on.
pub(crate) fn initialize() -> std::io::Result<PathBuf> {
    #[cfg(feature = "browser-package")]
    iptv_presentation::i18n::set_catalog_reader(package_catalog);
    resolve_data_dir()
}

/// Language `tag`'s messages, `i18n/<tag>.tsv` in the package
/// (`hosts/tizen-nsh/build.sh` writes them from the catalog).
#[cfg(feature = "browser-package")]
fn package_catalog(tag: &str) -> Option<String> {
    let bytes = iptv_web::adapters::package_file(&format!("i18n/{tag}.tsv"))?;
    String::from_utf8(bytes).ok()
}

thread_local! {
    /// The root owner's handle, for the page's lifecycle (step 2) and the
    /// headless helpers.
    static HOST_STOP: std::cell::RefCell<Option<crate::runtime::HostHandle>> =
        const { std::cell::RefCell::new(None) };
}

/// Keeps the root owner's handle. The page's lifecycle does not flush
/// through it yet (step 2).
pub(crate) fn flush_on_host_stop(owner: crate::runtime::HostHandle) {
    HOST_STOP.with(|stop| *stop.borrow_mut() = Some(owner));
}

/// The root owner's handle (headless helpers).
#[cfg(feature = "test-support")]
pub(crate) fn host_handle() -> Option<crate::runtime::HostHandle> {
    HOST_STOP.with(|stop| stop.borrow().clone())
}

/// Time since the Unix epoch on the host's wall clock.
pub(crate) fn since_epoch() -> Option<Duration> {
    Some(iptv_web::adapters::wall())
}

/// The device's offset from UTC at `unix` (the host's time zone).
pub(crate) fn offset_minutes(unix: i64) -> i32 {
    iptv_web::adapters::utc_offset_minutes(unix)
}

pub(crate) fn reload_time_zone() {}

/// The Worker's clock starts with the page; marks count from the first.
pub(crate) fn process_age() -> Option<Duration> {
    None
}

pub(crate) fn log_library_base() {}

/// Recording is unavailable here; its settings say so.
pub(crate) fn recording_command(_key: &str) {
    crate::settings_view::notify(crate::i18n::tr("Recording unavailable"));
}

/// An image's size from its header (NSH's own probe is native only).
pub(crate) fn image_extent(bytes: &[u8]) -> super::Extent {
    match imagesize::blob_size(bytes) {
        Ok(size) => super::Extent::Pixels(
            u32::try_from(size.width).unwrap_or(u32::MAX),
            u32::try_from(size.height).unwrap_or(u32::MAX),
        ),
        Err(_) if nsh_core::svg::sniff(bytes) => super::Extent::Vector,
        Err(_) => super::Extent::Unknown,
    }
}

/// One thread: `work` runs now, before this returns.
pub(crate) fn background(_name: &str, work: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    work();
    Ok(())
}

pub(crate) const NOTIFICATIONS_FOLLOW_LANGUAGE: bool = false;

pub(crate) fn language_changed(_tag: &str) {}

/// A browser host registers no native components: the page implements
/// `tv.player`.
pub mod exports {}

pub(crate) const EXTERNAL_PLAYER: bool = false;
#[derive(Clone)]
pub(crate) struct ExternalSession;
impl ExternalSession {
    pub(crate) fn close(&self) {}
    pub(crate) fn is_open(&self) -> bool {
        false
    }
    pub(crate) fn is_retired(&self) -> bool {
        true
    }
    pub(crate) async fn close_and_wait(&self) {}
    pub(crate) async fn wait_retired(&self) {}
}
pub(crate) struct ExternalHandoff;
impl ExternalHandoff {
    pub(crate) async fn connected(&mut self) -> bool {
        false
    }
    pub(crate) fn is_open(&self) -> bool {
        false
    }
    pub(crate) fn allow(self, _lease: Box<dyn Send>) -> Option<ExternalSession> {
        None
    }
}
pub(crate) async fn prepare_external_player(
    _source: iptv_core::playback::PlaySource,
) -> Option<()> {
    None
}
pub(crate) fn open_external_player(_playback: ()) -> Option<ExternalHandoff> {
    None
}
pub(crate) fn external_player_returned() {}

//! The browser TV host (Samsung Tizen first): the whole app in one Web
//! Worker, which the page drives (keys in, drawing and the player out).
//!
//! A stub until the NSH TV platform runs this crate (step 2 of
//! `docs/plans/2026-11-21-browser-tv-architecture.md`). What it builds is
//! real: the runtime's ports over `iptv-web`'s adapters, the page's fetch
//! for the network and the cooperative executor over the Worker's event
//! loop. What it does not have yet, it says so instead of pretending:
//!
//! - No key store: the database key broker refuses. The browser store opens
//!   without a key on memory files the host persists (`iptv-store-sqlite`'s
//!   `Store::open_browser`); the runtime's browser opening path is still to
//!   be wired, so until then the store reports itself unavailable.
//! - No entropy import yet: operations that need randomness fail
//!   (`Ports::entropy`'s default).
//! - The local time zone is UTC until the host reports its offset.
//! - Recording, backup files and staged probes are unavailable (the
//!   runtime's capability model reports them so).
//! - Artwork headers are not probed before the page decodes them.
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iptv_import::{BodyStream, Fetch, FetchError, StreamingResponse};
use iptv_providers::{HttpRequest, HttpResponse};
use iptv_web::adapters::{HostBody, HostFetch};

use crate::runtime::execution::{Cooperative, CooperativeHost, Task};
use crate::runtime::network::{Body, Chunks, Client, NetFuture, Network, NetworkConfig, Transport};

/// A browser TV is none of the native platforms: one decoder, no
/// background work, no OS registration (`iptv_core::capabilities::Host`).
pub(crate) const PLATFORM: iptv_core::capabilities::Platform =
    iptv_core::capabilities::Platform::Unknown;

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
    ports
}

/// The page's fetch, through the Worker's host.
struct PageNetwork;

impl Network for PageNetwork {
    fn client(&self, config: &NetworkConfig) -> Result<Client, FetchError> {
        Ok(Client::new(Page(HostFetch::new()), config.clone()))
    }
}

struct Page(HostFetch);

impl Transport for Page {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }

    fn sleep(&self, duration: Duration) -> NetFuture<'_, ()> {
        Box::pin(self.0.sleep(duration))
    }

    fn fetch(&self, request: HttpRequest) -> NetFuture<'_, Result<HttpResponse, FetchError>> {
        Box::pin(self.0.fetch(request))
    }

    fn fetch_stream(
        &self,
        request: HttpRequest,
    ) -> NetFuture<'_, Result<StreamingResponse<Body>, FetchError>> {
        Box::pin(async move {
            let response = self.0.fetch_stream(request).await?;
            Ok(StreamingResponse {
                status: response.status,
                content_length: response.content_length,
                body: Body::new(PageBody(response.body)),
            })
        })
    }
}

struct PageBody(HostBody);

impl Chunks for PageBody {
    fn next_chunk(&mut self) -> NetFuture<'_, Result<Option<Vec<u8>>, FetchError>> {
        Box::pin(self.0.next_chunk())
    }
}

/// The Worker's event loop: `iptv-web`'s executor, polled after every
/// delivery from the host.
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
pub(crate) fn initialize() -> std::io::Result<PathBuf> {
    resolve_data_dir()
}

/// The page's lifecycle does not flush yet (step 2).
pub(crate) fn flush_on_host_stop(_owner: crate::runtime::HostHandle) {}

/// Time since the Unix epoch on the host's wall clock.
pub(crate) fn since_epoch() -> Option<Duration> {
    Some(iptv_web::adapters::wall())
}

/// UTC until the host reports the device's offset.
pub(crate) fn offset_minutes(_unix: i64) -> i32 {
    0
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

/// NSH's header probe is native only. The page's image element decodes
/// artwork with the browser's own limits.
pub(crate) fn image_extent(_bytes: &[u8]) -> super::Extent {
    super::Extent::Unchecked
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

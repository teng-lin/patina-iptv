//! A single-threaded executor and the [`Fetch`] adapter over the host.
//!
//! Tasks are polled whenever the host delivers something (`poll` after each
//! export call) and whenever one is woken. A future waiting on the host
//! registers its waker with its slot, and each delivery wakes it, so the
//! adapter's futures also run under another executor (NSH's).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use iptv_import::{BodyStream, Fetch, FetchError, FetchErrorKind, StreamingResponse};
use iptv_providers::{HttpRequest, HttpResponse, Method};

use crate::host::host;

type Task = Pin<Box<dyn Future<Output = ()>>>;

thread_local! {
    static TASKS: RefCell<Vec<Task>> = const { RefCell::new(Vec::new()) };
    static SPAWNED: RefCell<Vec<Task>> = const { RefCell::new(Vec::new()) };
    static POLLING: Cell<bool> = const { Cell::new(false) };
    /// A task was woken, or one spawned, since the current round began.
    static WOKEN: Cell<bool> = const { Cell::new(false) };
    /// A wake-up poll is already asked of the host.
    static SCHEDULED: Cell<bool> = const { Cell::new(false) };
    /// A delivery is being applied: the export that applies it polls next.
    static DELIVERING: Cell<bool> = const { Cell::new(false) };
    static SLOTS: RefCell<HashMap<u32, Slot>> = RefCell::new(HashMap::new());
    static NEXT_ID: Cell<u32> = const { Cell::new(1) };
}

/// Rounds one [`poll`] runs at most before it leaves the rest to the next
/// one: a task that wakes itself on every poll must not hold the thread.
const ROUNDS: usize = 64;

/// The waker of every task: it marks work as runnable and, outside a
/// poll, asks the host for one (a zero-length timer, whose delivery polls).
struct Wake;

impl std::task::Wake for Wake {
    fn wake(self: std::sync::Arc<Self>) {
        woken();
    }
}

fn woken() {
    WOKEN.with(|w| w.set(true));
    // A poll follows a delivery (the export polls after it) and ends one:
    // only a wake from elsewhere asks the host for one.
    if !POLLING.with(Cell::get) && !DELIVERING.with(Cell::get) {
        schedule();
    }
}

/// Asks the host to call back soon, which polls.
fn schedule() {
    if SCHEDULED.with(Cell::get) {
        return;
    }
    // Only a timer actually asked for counts: without a host yet, the next
    // wake asks again.
    if let Some(host) = crate::host::host_for_wake() {
        SCHEDULED.with(|s| s.set(true));
        host.sleep(next_id(), 0.0);
    }
}

/// Runs `task` from the next [`poll`] on (one is asked of the host when
/// none is running).
pub(crate) fn spawn(task: impl Future<Output = ()> + 'static) {
    SPAWNED.with(|s| s.borrow_mut().push(Box::pin(task)));
    woken();
}

/// Polls every task until none makes progress without a new delivery.
pub(crate) fn poll() {
    if POLLING.with(|p| p.replace(true)) {
        // A task's host call answered synchronously: the outer poll runs
        // another round for what this delivery woke.
        WOKEN.with(|w| w.set(true));
        return;
    }
    SCHEDULED.with(|s| s.set(false));
    let waker = Waker::from(std::sync::Arc::new(Wake));
    let mut cx = Context::from_waker(&waker);
    // Every task runs until it waits; while a task was woken or spawned
    // during a round, another round follows.
    let mut rounds = 0;
    loop {
        WOKEN.with(|w| w.set(false));
        let mut tasks = TASKS.with(|t| std::mem::take(&mut *t.borrow_mut()));
        tasks.extend(SPAWNED.with(|s| std::mem::take(&mut *s.borrow_mut())));
        tasks.retain_mut(|t| t.as_mut().poll(&mut cx).is_pending());
        TASKS.with(|t| t.borrow_mut().extend(tasks));
        rounds += 1;
        if !WOKEN.with(Cell::get) {
            break;
        }
        if rounds == ROUNDS {
            POLLING.with(|p| p.set(false));
            schedule();
            return;
        }
    }
    POLLING.with(|p| p.set(false));
}

#[cfg(all(test, feature = "bridge"))]
pub(crate) fn pending_tasks() -> usize {
    TASKS.with(|t| t.borrow().len()) + SPAWNED.with(|s| s.borrow().len())
}

/// What the host has delivered for one request.
#[derive(Default)]
struct Slot {
    /// The task waiting on this slot, woken by its next delivery.
    waker: Option<Waker>,
    status: Option<(u16, Option<u64>)>,
    body: Option<Vec<u8>>,
    /// `Some(None)`: the stream ended.
    chunk: Option<Option<Vec<u8>>>,
    waiting_chunk: bool,
    error: Option<FetchError>,
    woke: bool,
    /// The request's scheme was HTTPS (a redirect must not downgrade it).
    https: bool,
    /// Content-supplied: every hop must be a public address.
    public_only: bool,
    /// The origin the request was configured for: a hop to another one
    /// must be public, as natively.
    origin: Option<String>,
}

fn next_id() -> u32 {
    NEXT_ID.with(|n| {
        let id = n.get();
        n.set(id.wrapping_add(1).max(1));
        id
    })
}

fn with_slot<R>(id: u32, f: impl FnOnce(&mut Slot) -> R) -> Option<R> {
    SLOTS.with(|s| s.borrow_mut().get_mut(&id).map(f))
}

/// Applies a delivery to slot `id` and wakes the task waiting on it.
fn deliver<R>(id: u32, f: impl FnOnce(&mut Slot) -> R) -> Option<R> {
    let (result, waker) = SLOTS.with(|s| {
        let mut slots = s.borrow_mut();
        let slot = slots.get_mut(&id)?;
        let result = f(slot);
        Some((result, slot.waker.take()))
    })?;
    if let Some(waker) = waker {
        let was = DELIVERING.with(|d| d.replace(true));
        waker.wake();
        DELIVERING.with(|d| d.set(was));
    }
    Some(result)
}

/// Remembers `cx`'s waker for slot `id`'s next delivery.
fn register(id: u32, cx: &Context<'_>) {
    with_slot(id, |s| match &s.waker {
        Some(waker) if waker.will_wake(cx.waker()) => {}
        _ => s.waker = Some(cx.waker().clone()),
    });
}

fn open_slot(id: u32) {
    SLOTS.with(|s| s.borrow_mut().insert(id, Slot::default()));
}

/// Owns a slot while a future waits on it. Dropped before the host
/// answered (an import canceled mid-request), it frees the slot and, for a
/// request, tells the host to abort it so no connection stays open.
struct Pending {
    id: u32,
    request: bool,
    done: bool,
}

impl Pending {
    fn new(id: u32, request: bool) -> Self {
        open_slot(id);
        Self {
            id,
            request,
            done: false,
        }
    }
    /// The slot now belongs to someone else (a streamed body).
    fn hand_over(mut self) {
        self.done = true;
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        if self.request
            && let Some(host) = crate::host::host_if_alive()
        {
            host.fetch_close(self.id);
        }
        close_slot(self.id);
    }
}

fn close_slot(id: u32) {
    // try_with: also called from destructors while the thread shuts down.
    let _ = SLOTS.try_with(|s| s.borrow_mut().remove(&id));
}

// Deliveries from the host (the exports call these).

pub(crate) fn deliver_status(id: u32, status: u16, length: Option<u64>) {
    deliver(id, |s| s.status = Some((status, length)));
}

pub(crate) fn deliver_body(id: u32, bytes: Vec<u8>) {
    deliver(id, |s| s.body = Some(bytes));
}

pub(crate) fn deliver_chunk(id: u32, bytes: Option<Vec<u8>>) {
    deliver(id, |s| {
        s.chunk = Some(bytes);
        s.waiting_chunk = false;
    });
}

pub(crate) fn deliver_error(id: u32, kind: u32, detail: String) {
    let kind = match kind {
        1 => FetchErrorKind::Connect,
        2 => FetchErrorKind::Timeout,
        3 => FetchErrorKind::Body,
        4 => FetchErrorKind::InvalidRequest,
        5 => FetchErrorKind::TooLarge,
        7 => FetchErrorKind::Deadline,
        _ => FetchErrorKind::Other,
    };
    deliver(id, |s| s.error = Some(FetchError::new(kind, detail)));
}

/// The browser followed a redirect to `url` before answering request `id`.
/// A downgrade to HTTP, or a non-public hop of a content-supplied request,
/// fails the request and aborts it.
pub(crate) fn deliver_redirect(id: u32, url: &str) {
    let refused = deliver(id, |s| {
        let downgrade = s.https && !url.trim_start().to_ascii_lowercase().starts_with("https:");
        let elsewhere = origin(url) != s.origin;
        let private =
            (s.public_only || elsewhere) && iptv_providers::content_url::validate(url).is_err();
        if downgrade || private {
            let why = if downgrade {
                "redirect to plain HTTP refused"
            } else {
                "redirect to a non-public address refused"
            };
            s.error = Some(FetchError::new(FetchErrorKind::Redirect, why));
            true
        } else {
            false
        }
    });
    if refused == Some(true) {
        host().fetch_close(id);
    }
}

pub(crate) fn deliver_wake(id: u32) {
    deliver(id, |s| s.woke = true);
}

/// A stored value arrived (`None`: there is none).
pub(crate) fn deliver_stored(id: u32, value: Option<Vec<u8>>) {
    deliver(id, |s| {
        s.chunk = Some(value);
    });
}

/// Reads `key` from the host's storage.
pub(crate) async fn storage_get(key: &str) -> Option<Vec<u8>> {
    let pending = Pending::new(next_id(), false);
    let id = pending.id;
    host().store_get(id, key);
    std::future::poll_fn(|cx| {
        register(id, cx);
        match with_slot(id, |s| s.chunk.take()) {
            Some(Some(v)) => Poll::Ready(v),
            Some(None) => Poll::Pending,
            None => Poll::Ready(None),
        }
    })
    .await
}

/// Writes `value` under `key`; writes keep their order. Empty deletes.
pub(crate) fn storage_put(key: &str, value: &[u8]) {
    host().store_put(key, value);
}

/// The request as the host reads it.
///
/// `redirect` is `error` for a request that carries a header another origin
/// must not receive (anything outside
/// [`iptv_providers::http::CROSS_ORIGIN_HEADERS`]): the page's fetch would
/// forward it to wherever a redirect leads, where the native adapter drops
/// it. Every other request follows; the final address is then checked
/// (`deliver_redirect`): public when the request was content-supplied or
/// the origin changed, never a downgrade. Hops before the last reach their
/// address before Rust sees them (`docs/security.md`). The Live TV
/// bridge's worker always follows.
fn request_json(request: &HttpRequest, idle_ms: u64) -> String {
    let headers: Vec<serde_json::Value> = request
        .headers
        .iter()
        .map(|(k, v)| serde_json::json!([k, v]))
        .collect();
    serde_json::json!({
        "url": request.url,
        "method": match request.method { Method::Get => "GET", Method::Post => "POST" },
        "headers": headers,
        "body": request.body,
        "timeoutMs": request.timeout.map(|t| t.as_millis() as u64),
        "idleMs": idle_ms,
        "redirect": if origin_bound(request) { "error" } else { "follow" },
    })
    .to_string()
}

/// `request` carries a header that belongs to its origin.
fn origin_bound(request: &HttpRequest) -> bool {
    request.headers.iter().any(|(name, _)| {
        !iptv_providers::http::CROSS_ORIGIN_HEADERS
            .iter()
            .any(|kept| name.eq_ignore_ascii_case(kept))
    })
}

/// `scheme://host:port` of `address`, when it is one.
fn origin(address: &str) -> Option<String> {
    let url = url::Url::parse(address.trim()).ok()?;
    Some(url.origin().ascii_serialization())
}

/// A download that receives nothing for this long fails (the native
/// adapter's read timeout); the import's own deadline bounds the total.
const IDLE_TIMEOUT_MS: u64 = 60_000;

/// Whether `url` may be opened: any HTTP(S) address the user chose, and
/// only public ones when the address came from content
/// (`iptv_providers::content_url`). The browser resolves names itself, so a
/// public name that resolves to a LAN address cannot be refused here; see
/// the crate documentation.
pub(crate) fn allowed(url: &str, public_only: bool) -> bool {
    if public_only {
        iptv_providers::content_url::validate(url).is_ok()
    } else {
        iptv_providers::content_url::validate_picked(url).is_ok()
    }
}

pub(crate) fn check_destination(request: &HttpRequest) -> Result<(), FetchError> {
    if allowed(&request.url, request.public_only) {
        Ok(())
    } else if request.public_only {
        Err(FetchError::new(
            FetchErrorKind::InvalidRequest,
            "a content-supplied address must be public",
        ))
    } else {
        Err(FetchError::new(
            FetchErrorKind::Unsupported,
            "only HTTP(S) addresses are fetched",
        ))
    }
}

/// [`Fetch`] through the JavaScript host.
#[derive(Clone)]
pub struct HostFetch {
    started_ms: f64,
    idle_ms: u64,
}

impl Default for HostFetch {
    fn default() -> Self {
        Self::new()
    }
}

impl HostFetch {
    pub fn new() -> Self {
        Self {
            started_ms: host().monotonic_ms(),
            idle_ms: IDLE_TIMEOUT_MS,
        }
    }
    /// A download that receives nothing for `idle` fails.
    pub fn with_idle_timeout(mut self, idle: Duration) -> Self {
        self.idle_ms = u64::try_from(idle.as_millis()).unwrap_or(u64::MAX);
        self
    }

    async fn status(
        &self,
        request: &HttpRequest,
        stream: bool,
    ) -> Result<(Pending, u16, Option<u64>), FetchError> {
        check_destination(request)?;
        let pending = Pending::new(next_id(), true);
        let id = pending.id;
        with_slot(id, |s| {
            s.https = request
                .url
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("https:");
            s.public_only = request.public_only;
            s.origin = origin(&request.url);
        });
        host().fetch(id, &request_json(request, self.idle_ms), stream);
        let (status, length) = std::future::poll_fn(|cx| {
            register(id, cx);
            match with_slot(id, |s| {
                if let Some(e) = s.error.take() {
                    Some(Err(e))
                } else {
                    s.status.map(Ok)
                }
            }) {
                Some(Some(r)) => Poll::Ready(r),
                Some(None) => Poll::Pending,
                None => Poll::Ready(Err(FetchError::new(FetchErrorKind::Other, "request lost"))),
            }
        })
        .await?;
        Ok((pending, status, length))
    }
}

impl Fetch for HostFetch {
    type Body = HostBody;

    fn elapsed(&self) -> Duration {
        Duration::from_secs_f64(((host().monotonic_ms() - self.started_ms) / 1000.0).max(0.0))
    }

    async fn sleep(&self, duration: Duration) {
        sleep_while(duration, || true).await;
    }

    async fn fetch(&self, request: HttpRequest) -> Result<HttpResponse, FetchError> {
        let (mut pending, status, _) = self.status(&request, false).await?;
        let id = pending.id;
        let body = std::future::poll_fn(|cx| {
            register(id, cx);
            match with_slot(id, |s| {
                if let Some(e) = s.error.take() {
                    Some(Err(e))
                } else {
                    s.body.take().map(Ok)
                }
            }) {
                Some(Some(r)) => Poll::Ready(r),
                Some(None) => Poll::Pending,
                None => Poll::Ready(Err(FetchError::new(FetchErrorKind::Body, "request lost"))),
            }
        })
        .await;
        // Answered (body or error): nothing is left to abort.
        pending.request = false;
        drop(pending);
        Ok(HttpResponse::new(status, body?))
    }

    async fn fetch_stream(
        &self,
        request: HttpRequest,
    ) -> Result<StreamingResponse<HostBody>, FetchError> {
        let (pending, status, content_length) = self.status(&request, true).await?;
        let id = pending.id;
        pending.hand_over();
        Ok(StreamingResponse {
            status,
            same_request_origin: false,
            content_length,
            body: HostBody {
                id,
                done: Rc::new(Cell::new(false)),
            },
        })
    }
}

/// Waits `duration`, or less: it ends as soon as `keep()` is false (checked
/// whenever the executor polls), so a superseded task stops at the next
/// delivery instead of holding its state until the timer fires.
pub(crate) async fn sleep_while(duration: Duration, keep: impl Fn() -> bool) {
    let pending = Pending::new(next_id(), false);
    let id = pending.id;
    // setTimeout holds a signed 32-bit count of milliseconds.
    host().sleep(
        id,
        (duration.as_secs_f64() * 1000.0).min(f64::from(i32::MAX)),
    );
    std::future::poll_fn(|cx| {
        register(id, cx);
        if !keep() || with_slot(id, |s| s.woke).unwrap_or(true) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
}

/// The body of a streamed response, read chunk by chunk from the host.
pub struct HostBody {
    id: u32,
    done: Rc<Cell<bool>>,
}

impl BodyStream for HostBody {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, FetchError> {
        if self.done.get() {
            return Ok(None);
        }
        let id = self.id;
        let ask = with_slot(id, |s| {
            if s.chunk.is_none() && !s.waiting_chunk {
                s.waiting_chunk = true;
                true
            } else {
                false
            }
        });
        if ask == Some(true) {
            host().fetch_next(id);
        }
        let got = std::future::poll_fn(|cx| {
            register(id, cx);
            match with_slot(id, |s| {
                if let Some(e) = s.error.take() {
                    Some(Err(e))
                } else {
                    s.chunk.take().map(Ok)
                }
            }) {
                Some(Some(r)) => Poll::Ready(r),
                Some(None) => Poll::Pending,
                None => Poll::Ready(Err(FetchError::new(FetchErrorKind::Body, "stream lost"))),
            }
        })
        .await;
        match got {
            Ok(Some(bytes)) => Ok(Some(bytes)),
            Ok(None) => {
                self.done.set(true);
                Ok(None)
            }
            Err(e) => {
                self.done.set(true);
                Err(e)
            }
        }
    }
}

impl Drop for HostBody {
    fn drop(&mut self) {
        if !self.done.get()
            && let Some(host) = crate::host::host_if_alive()
        {
            host.fetch_close(self.id);
        }
        close_slot(self.id);
    }
}

/// The host answered chunk save `id` (`on_chunks_saved`).
pub(crate) fn deliver_saved(id: u32, ok: bool) {
    deliver(id, |s| s.status = Some((u16::from(ok), None)));
}

/// A chunk save the host was handed and has not answered yet.
pub(crate) struct Saving(Pending);

impl Saving {
    pub(crate) fn new() -> Self {
        Self(Pending::new(next_id(), false))
    }

    pub(crate) fn id(&self) -> u32 {
        self.0.id
    }

    /// True once the host's transaction committed; false when it failed.
    pub(crate) async fn answer(self) -> bool {
        let id = self.0.id;
        let _pending = self.0;
        std::future::poll_fn(|cx| {
            register(id, cx);
            match with_slot(id, |s| s.status.take()) {
                Some(Some((ok, _))) => Poll::Ready(ok == 1),
                Some(None) => Poll::Pending,
                None => Poll::Ready(false),
            }
        })
        .await
    }
}

#[cfg(test)]
mod request_tests {
    use super::*;

    #[test]
    fn a_request_with_origin_headers_refuses_redirects_the_page_would_follow() {
        let redirect = |request: &HttpRequest| {
            let json: serde_json::Value =
                serde_json::from_str(&request_json(request, 15_000)).unwrap();
            assert_eq!(json["idleMs"], 15_000);
            json["redirect"].as_str().unwrap().to_owned()
        };
        let mut logo = HttpRequest::get("https://example.test/logo.png");
        logo.public_only = true;
        assert_eq!(redirect(&logo), "follow", "a redirected logo still loads");
        let negotiated = HttpRequest::get("https://example.test/list.m3u")
            .with_header("User-Agent", "Synthetic/1")
            .with_header("Range", "bytes=0-");
        assert_eq!(redirect(&negotiated), "follow");
        let keyed =
            HttpRequest::get("https://example.test/list.m3u").with_header("X-Api-Key", "synthetic");
        assert_eq!(redirect(&keyed), "error");
    }
}

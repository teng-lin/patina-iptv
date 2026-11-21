//! A single-threaded executor and the [`Fetch`] adapter over the host.
//!
//! Tasks are polled whenever the host delivers something (`poll` after each
//! export call), so no waker has to schedule anything: a pending future
//! simply waits for the next delivery.

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
    static SLOTS: RefCell<HashMap<u32, Slot>> = RefCell::new(HashMap::new());
    static NEXT_ID: Cell<u32> = const { Cell::new(1) };
}

/// Runs `task` from the next [`poll`] on.
pub(crate) fn spawn(task: impl Future<Output = ()> + 'static) {
    SPAWNED.with(|s| s.borrow_mut().push(Box::pin(task)));
}

/// Polls every task until none makes progress without a new delivery.
pub(crate) fn poll() {
    if POLLING.with(|p| p.replace(true)) {
        return; // A task's host call answered synchronously: the outer poll continues.
    }
    let mut cx = Context::from_waker(Waker::noop());
    // Every task runs until it waits on the host; a task spawned meanwhile
    // gets its first poll in the next round.
    loop {
        let mut tasks = TASKS.with(|t| std::mem::take(&mut *t.borrow_mut()));
        tasks.extend(SPAWNED.with(|s| std::mem::take(&mut *s.borrow_mut())));
        tasks.retain_mut(|t| t.as_mut().poll(&mut cx).is_pending());
        TASKS.with(|t| *t.borrow_mut() = tasks);
        if SPAWNED.with(|s| s.borrow().is_empty()) {
            break;
        }
    }
    POLLING.with(|p| p.set(false));
}

#[cfg(test)]
pub(crate) fn pending_tasks() -> usize {
    TASKS.with(|t| t.borrow().len()) + SPAWNED.with(|s| s.borrow().len())
}

/// What the host has delivered for one request.
#[derive(Default)]
struct Slot {
    status: Option<(u16, Option<u64>)>,
    body: Option<Vec<u8>>,
    /// `Some(None)`: the stream ended.
    chunk: Option<Option<Vec<u8>>>,
    waiting_chunk: bool,
    error: Option<FetchError>,
    woke: bool,
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

fn open_slot(id: u32) {
    SLOTS.with(|s| s.borrow_mut().insert(id, Slot::default()));
}

fn close_slot(id: u32) {
    SLOTS.with(|s| s.borrow_mut().remove(&id));
}

// Deliveries from the host (the exports call these).

pub(crate) fn deliver_status(id: u32, status: u16, length: Option<u64>) {
    with_slot(id, |s| s.status = Some((status, length)));
}

pub(crate) fn deliver_body(id: u32, bytes: Vec<u8>) {
    with_slot(id, |s| s.body = Some(bytes));
}

pub(crate) fn deliver_chunk(id: u32, bytes: Option<Vec<u8>>) {
    with_slot(id, |s| {
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
        _ => FetchErrorKind::Other,
    };
    with_slot(id, |s| s.error = Some(FetchError::new(kind, detail)));
}

pub(crate) fn deliver_wake(id: u32) {
    with_slot(id, |s| s.woke = true);
}

/// A stored value arrived (`None`: there is none).
pub(crate) fn deliver_stored(id: u32, value: Option<Vec<u8>>) {
    with_slot(id, |s| {
        s.chunk = Some(value);
    });
}

/// Reads `key` from the host's storage.
pub(crate) async fn storage_get(key: &str) -> Option<Vec<u8>> {
    let id = next_id();
    open_slot(id);
    host().store_get(id, key);
    let value = std::future::poll_fn(|_| match with_slot(id, |s| s.chunk.take()) {
        Some(Some(v)) => Poll::Ready(v),
        Some(None) => Poll::Pending,
        None => Poll::Ready(None),
    })
    .await;
    close_slot(id);
    value
}

/// Writes `value` under `key`; writes keep their order. Empty deletes.
pub(crate) fn storage_put(key: &str, value: &[u8]) {
    host().store_put(key, value);
}

/// The request as the host reads it.
fn request_json(request: &HttpRequest) -> String {
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
    })
    .to_string()
}

/// Content-supplied destinations must be public. The browser resolves names
/// itself, so only literal addresses and local names can be refused here;
/// see the crate documentation.
pub(crate) fn check_destination(request: &HttpRequest) -> Result<(), FetchError> {
    let url = url::Url::parse(&request.url)
        .map_err(|_| FetchError::new(FetchErrorKind::InvalidRequest, "not a URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(FetchError::new(
            FetchErrorKind::Unsupported,
            "only HTTP(S) is fetched",
        ));
    }
    if !request.public_only {
        return Ok(());
    }
    let private = match url.host() {
        Some(url::Host::Ipv4(ip)) => !iptv_providers::content_url::public_ip(ip.into()),
        Some(url::Host::Ipv6(ip)) => !iptv_providers::content_url::public_ip(ip.into()),
        Some(url::Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            d == "localhost"
                || d.ends_with(".localhost")
                || d.ends_with(".local")
                || !d.contains('.')
        }
        None => true,
    };
    if private {
        Err(FetchError::new(
            FetchErrorKind::InvalidRequest,
            "a content-supplied address must be public",
        ))
    } else {
        Ok(())
    }
}

/// [`Fetch`] through the JavaScript host.
#[derive(Clone)]
pub(crate) struct HostFetch {
    started_ms: f64,
}

impl HostFetch {
    pub(crate) fn new() -> Self {
        Self {
            started_ms: host().monotonic_ms(),
        }
    }

    async fn status(
        &self,
        request: &HttpRequest,
        stream: bool,
    ) -> Result<(u32, u16, Option<u64>), FetchError> {
        check_destination(request)?;
        let id = next_id();
        open_slot(id);
        host().fetch(id, &request_json(request), stream);
        let got = std::future::poll_fn(|_| {
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
        .await;
        match got {
            Ok((status, length)) => Ok((id, status, length)),
            Err(e) => {
                close_slot(id);
                Err(e)
            }
        }
    }
}

impl Fetch for HostFetch {
    type Body = HostBody;

    fn elapsed(&self) -> Duration {
        Duration::from_secs_f64(((host().monotonic_ms() - self.started_ms) / 1000.0).max(0.0))
    }

    async fn sleep(&self, duration: Duration) {
        let id = next_id();
        open_slot(id);
        host().sleep(id, duration.as_secs_f64() * 1000.0);
        std::future::poll_fn(|_| {
            if with_slot(id, |s| s.woke).unwrap_or(true) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        close_slot(id);
    }

    async fn fetch(&self, request: HttpRequest) -> Result<HttpResponse, FetchError> {
        let (id, status, _) = self.status(&request, false).await?;
        let body = std::future::poll_fn(|_| {
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
        close_slot(id);
        Ok(HttpResponse::new(status, body?))
    }

    async fn fetch_stream(
        &self,
        request: HttpRequest,
    ) -> Result<StreamingResponse<HostBody>, FetchError> {
        let (id, status, content_length) = self.status(&request, true).await?;
        Ok(StreamingResponse {
            status,
            content_length,
            body: HostBody {
                id,
                done: Rc::new(Cell::new(false)),
            },
        })
    }
}

/// The body of a streamed response, read chunk by chunk from the host.
pub(crate) struct HostBody {
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
        let got = std::future::poll_fn(|_| {
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
        if !self.done.get() {
            host().fetch_close(self.id);
        }
        close_slot(self.id);
    }
}

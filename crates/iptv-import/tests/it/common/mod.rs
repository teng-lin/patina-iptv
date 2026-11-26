//! In-memory `Fetch` fake and a minimal executor for the import tests.
#![allow(dead_code)]

pub mod synth;

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::Thread;
use std::time::Duration;

use iptv_import::{
    CancelToken, Fetch, FetchError, FetchErrorKind, MemoryBody, Progress, StreamingResponse,
};
use iptv_providers::{HttpRequest, HttpResponse};

use synth::{SimRequest, SimResponse};

/// Runs a future to completion on this thread (wakers unpark it).
pub fn block_on<F: Future>(fut: F) -> F::Output {
    struct Unpark(Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park();
    }
}

/// What the fake answers.
pub enum Reply {
    Http(SimResponse),
    /// A body that breaks after its bytes (connection dropped).
    Truncated(SimResponse),
    Error(FetchError),
    /// Never completes; optionally cancels `token` from another thread first.
    Hang(Option<CancelToken>),
    /// Answers after this much time on the fake's virtual clock.
    Slow(Duration, SimResponse),
}

impl From<SimResponse> for Reply {
    fn from(r: SimResponse) -> Self {
        Reply::Http(r)
    }
}

type Handler = Box<dyn Fn(&HttpRequest, &SimRequest) -> Reply>;

/// A `Fetch` answered by a closure. Logs every request. Its clock is
/// virtual: only [`Reply::Slow`] and `Fetch::sleep` move it, and neither
/// waits.
pub struct FakeFetch {
    handler: Handler,
    pub chunk: usize,
    pub same_request_origin: bool,
    pub log: RefCell<Vec<HttpRequest>>,
    pub clock: Cell<Duration>,
    /// Every wait the importer asked for, in order.
    pub slept: RefCell<Vec<Duration>>,
    /// Canceled when the importer starts a wait.
    pub cancel_on_sleep: RefCell<Option<CancelToken>>,
}

impl FakeFetch {
    pub fn new(handler: impl Fn(&HttpRequest, &SimRequest) -> Reply + 'static) -> Self {
        Self {
            handler: Box::new(handler),
            chunk: 7 * 1024 + 3,
            same_request_origin: true,
            log: RefCell::new(Vec::new()),
            clock: Cell::new(Duration::ZERO),
            slept: RefCell::new(Vec::new()),
            cancel_on_sleep: RefCell::new(None),
        }
    }

    /// Serves a fixed body for every URL.
    pub fn body(body: impl Into<Vec<u8>>) -> Self {
        let body = body.into();
        Self::new(move |_, _| SimResponse::ok(body.clone()).into())
    }

    pub fn requests(&self) -> Vec<HttpRequest> {
        self.log.borrow().clone()
    }

    /// The answer to `req`; a slow one has arrived by the time this returns.
    fn reply(&self, req: HttpRequest) -> Reply {
        let sim = SimRequest::parse(&req.url, req.headers.clone());
        let r = (self.handler)(&req, &sim);
        self.log.borrow_mut().push(req);
        match r {
            Reply::Slow(took, response) => {
                self.clock.set(self.clock.get() + took);
                Reply::Http(response)
            }
            r => r,
        }
    }
}

type Fut<'a, T> = Pin<Box<dyn Future<Output = Result<T, FetchError>> + 'a>>;

fn hang<'a, T: 'a>(token: Option<CancelToken>) -> Fut<'a, T> {
    if let Some(t) = token {
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            t.cancel();
        });
    }
    Box::pin(std::future::pending())
}

impl Fetch for FakeFetch {
    type Body = MemoryBody;

    fn elapsed(&self) -> Duration {
        self.clock.get()
    }

    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> {
        self.slept.borrow_mut().push(duration);
        self.clock.set(self.clock.get() + duration);
        if let Some(token) = self.cancel_on_sleep.borrow().as_ref() {
            token.cancel();
        }
        std::future::ready(())
    }

    fn fetch(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, FetchError>> {
        let fut: Fut<'_, HttpResponse> = match self.reply(request) {
            Reply::Http(r) | Reply::Slow(_, r) => {
                Box::pin(std::future::ready(Ok(HttpResponse::new(r.status, r.body))))
            }
            Reply::Truncated(_) => Box::pin(std::future::ready(Err(FetchError::new(
                FetchErrorKind::Body,
                "connection reset",
            )))),
            Reply::Error(e) => Box::pin(std::future::ready(Err(e))),
            Reply::Hang(t) => hang(t),
        };
        fut
    }

    fn fetch_stream(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<StreamingResponse<MemoryBody>, FetchError>> {
        let chunk = self.chunk;
        let fut: Fut<'_, StreamingResponse<MemoryBody>> = match self.reply(request) {
            Reply::Http(r) | Reply::Slow(_, r) => {
                Box::pin(std::future::ready(Ok(StreamingResponse {
                    status: r.status,
                    same_request_origin: self.same_request_origin,
                    content_length: Some(r.body.len() as u64),
                    body: MemoryBody::new(&r.body, chunk),
                })))
            }
            Reply::Truncated(r) => Box::pin(std::future::ready(Ok(StreamingResponse {
                status: r.status,
                same_request_origin: self.same_request_origin,
                content_length: None,
                body: MemoryBody::new(&r.body, chunk)
                    .failing_at_end(FetchError::new(FetchErrorKind::Body, "connection reset")),
            }))),
            Reply::Error(e) => Box::pin(std::future::ready(Err(e))),
            Reply::Hang(t) => hang(t),
        };
        fut
    }
}

/// Collects progress events.
#[derive(Default)]
pub struct Events(pub Vec<Progress>);

impl iptv_import::ProgressSink for Events {
    fn report(&mut self, p: &Progress) {
        self.0.push(p.clone());
    }
}

pub fn connect_error() -> Reply {
    Reply::Error(FetchError::new(
        FetchErrorKind::Connect,
        "connection refused",
    ))
}

//! One owned incoming MPEG-TS stream, shared by playback and capture.
//!
//! HLS is read sequentially using the recorder's existing bounded parser. Local
//! consumers see TS, never provider addresses or identity headers. The last
//! owner cancels ingestion and all local connections. Slow consumers fail with
//! a gap instead of blocking playback or silently losing recording bytes.
use crate::{
    playback::{PlaybackPolicy, PlaybackRelay},
    recording::{self, CaptureState, Failure, Sink},
};
use iptv_import::CancelToken;
use std::{
    collections::{BTreeMap, VecDeque},
    io,
    net::{Ipv4Addr, TcpListener},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{Notify, watch},
    task::AbortHandle,
};

const CHUNK: usize = 188 * 348;
const BUFFER: usize = 4 * 1024 * 1024;
const CLIENTS: usize = 12;
const PRE_ROLL: usize = 512 * 1024;

#[derive(Default)]
struct Buffer {
    chunks: VecDeque<(u64, Arc<[u8]>)>,
    next: u64,
    bytes: usize,
    ended: Option<Failure>,
    readers: BTreeMap<u64, u64>,
    reader_id: u64,
    pre_roll: Vec<u8>,
    pre_roll_valid: bool,
    pre_roll_at: Option<tokio::time::Instant>,
}
struct Shared {
    buffer: Mutex<Buffer>,
    changed: Notify,
}
impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Buffer> {
        self.buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
struct Owner {
    url: String,
    shared: Arc<Shared>,
    lifetime: CancelToken,
    closed: watch::Receiver<bool>,
    ready: watch::Receiver<Option<Result<(), Failure>>>,
    validated: watch::Receiver<Option<Result<(), Failure>>>,
    server: AbortHandle,
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.shared.lock().ended = Some(Failure::Network);
        self.shared.changed.notify_waiters();
        self.lifetime.cancel();
        self.server.abort();
    }
}
/// Clones retain the original incoming connection independently of any player.
#[derive(Clone)]
pub struct LiveInput(Arc<Owner>);
impl LiveInput {
    /// Returns only after its first segment (a TS stream's first packets)
    /// passed validation. Unsupported transports
    /// leave no live owner, allowing the caller's permitted independent path.
    /// Must run on the host's owned Tokio executor.
    pub async fn start(
        url: &str,
        headers: &[(String, String)],
        policy: PlaybackPolicy,
    ) -> Result<Self, Failure> {
        Self::start_with_cancel(url, headers, policy, CancelToken::new()).await
    }

    /// Cancellation is honored until the first media is accepted. Afterwards
    /// the input lifetime belongs to its owners, so a zap cannot stop capture.
    pub async fn start_with_cancel(
        url: &str,
        headers: &[(String, String)],
        policy: PlaybackPolicy,
        prepare_cancel: CancelToken,
    ) -> Result<Self, Failure> {
        let input = Self::begin_with_cancel(url, headers, policy, prepare_cancel)?;
        match input.first_segment().await {
            Ok(()) => Ok(input),
            Err(failure) => {
                input.shutdown().await;
                Err(failure)
            }
        }
    }

    /// Begin an owned preparation without awaiting provider I/O, allowing the
    /// host to register its original account lease before yielding control.
    pub fn begin_with_cancel(
        url: &str,
        headers: &[(String, String)],
        policy: PlaybackPolicy,
        prepare_cancel: CancelToken,
    ) -> Result<Self, Failure> {
        let relay =
            PlaybackRelay::start_with_policy(url, headers, policy).map_err(|_| Failure::Network)?;
        let client = recording::client()?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|_| Failure::Network)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| Failure::Network)?;
        let mut random = [0u8; 24];
        getrandom::fill(&mut random).map_err(|_| Failure::Network)?;
        let token: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let path = format!("/{token}/live.ts");
        let url = format!(
            "http://{}{path}",
            listener.local_addr().map_err(|_| Failure::Network)?
        );
        let shared = Arc::new(Shared {
            buffer: Mutex::default(),
            changed: Notify::new(),
        });
        let (ready, started) = watch::channel(None);
        let (validated, checked) = watch::channel(None);
        let first = Arc::new(AtomicBool::new(false));
        let lifetime = CancelToken::new();
        let ending = lifetime.clone();
        let (closed_tx, closed) = watch::channel(false);
        let prepared = first.clone();
        let mut sink = Fanout {
            shared: shared.clone(),
            ready: Some(ready),
            validated: Some(validated),
            pending: Vec::new(),
            first,
        };
        tokio::spawn(async move {
            let mut state = CaptureState::default();
            let cancelled_preparation = async {
                prepare_cancel.cancelled().await;
                if prepared.load(Ordering::Acquire) {
                    std::future::pending::<()>().await;
                }
            };
            let outcome = tokio::select! {
                biased;
                _ = ending.cancelled() => Err(Failure::Network),
                _ = cancelled_preparation => Err(Failure::Network),
                result = recording::receive(&client, &relay, &mut sink, &mut state) => result,
            };
            let failure = outcome.err().unwrap_or(Failure::Network);
            relay.shutdown().await;
            if let Some(ready) = sink.ready.take() {
                let _ = ready.send(Some(Err(failure)));
            }
            if let Some(validated) = sink.validated.take() {
                let _ = validated.send(Some(Err(failure)));
            }
            sink.shared.lock().ended = Some(failure);
            sink.shared.changed.notify_waiters();
            let _ = closed_tx.send(true);
        });
        let server_shared = Arc::downgrade(&shared);
        let server_cancel = lifetime.clone();
        let server = tokio::spawn(async move {
            let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                server_cancel.cancel();
                return;
            };
            serve_connections(|| listener.accept(), path, server_shared).await;
        });
        let owner = Self(Arc::new(Owner {
            url,
            shared,
            lifetime,
            closed,
            ready: started,
            validated: checked,
            server: server.abort_handle(),
        }));
        Ok(owner)
    }
    /// The first media packets arrived: a player may open the input now.
    /// An HLS segment's packets are taken as they arrive, so its first
    /// segment may still fail validation: see [`LiveInput::first_segment`].
    pub async fn ready(&self) -> Result<(), Failure> {
        outcome(self.0.ready.clone()).await
    }
    /// The first HLS segment arrived whole and passed validation (a TS
    /// stream: its first packets). An `Unsupported` or `Limit` failure means
    /// this stream cannot be shared and its provider's own transport should
    /// play it. Holds no owner: the input may end while this waits.
    pub fn first_segment(&self) -> impl Future<Output = Result<(), Failure>> + Send + 'static {
        outcome(self.0.validated.clone())
    }
    pub fn is_ready(&self) -> bool {
        *self.0.ready.borrow() == Some(Ok(()))
    }
    pub fn url(&self) -> &str {
        &self.0.url
    }
    pub fn is_running(&self) -> bool {
        self.0.shared.lock().ended.is_none()
    }
    /// Capture starts at the live edge with a bounded current PAT interval and retains
    /// the incoming owner itself. Slow readers report an explicit gap.
    pub fn subscribe(&self) -> LiveReader {
        let mut reader = LiveReader::new(self.0.shared.clone());
        reader.owner = Some(self.0.clone());
        reader
    }
    /// Retire an uncommitted input before opening its replacement.
    pub async fn shutdown(self) {
        self.0.lifetime.cancel();
        let mut closed = self.0.closed.clone();
        while !*closed.borrow_and_update() {
            if closed.changed().await.is_err() {
                break;
            }
        }
    }
    pub fn same_input(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// A watched preparation step's result, once it has one.
async fn outcome(mut step: watch::Receiver<Option<Result<(), Failure>>>) -> Result<(), Failure> {
    loop {
        if let Some(result) = *step.borrow_and_update() {
            return result;
        }
        step.changed().await.map_err(|_| Failure::Network)?;
    }
}

struct Fanout {
    shared: Arc<Shared>,
    ready: Option<watch::Sender<Option<Result<(), Failure>>>>,
    validated: Option<watch::Sender<Option<Result<(), Failure>>>>,
    pending: Vec<u8>,
    first: Arc<AtomicBool>,
}
impl Sink for Fanout {
    fn accepts_master_playlist(&self) -> bool {
        false
    }
    fn buffers_segments(&self) -> bool {
        false
    }
    fn segment_complete(&mut self) {
        if let Some(validated) = self.validated.take() {
            let _ = validated.send(Some(Ok(())));
        }
    }
    async fn write(&mut self, bytes: &[u8]) -> Result<(), Failure> {
        self.pending.extend_from_slice(bytes);
        let whole = self.pending.len() / 188 * 188;
        if !self.pending[..whole]
            .as_chunks::<188>()
            .0
            .iter()
            .all(|packet| packet[0] == 0x47)
        {
            return Err(Failure::Unsupported);
        }
        if whole == 0 {
            return Ok(());
        }
        for chunk in self.pending[..whole].chunks(CHUNK) {
            // Any advancing consumer keeps ingestion moving. Bound read-ahead
            // by the furthest cursor, so a stalled reader cannot stop healthy
            // playback or capture; laggards fall behind the retained window.
            loop {
                let notified = self.shared.changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let wait = {
                    let buffer = self.shared.lock();
                    match buffer.readers.values().max() {
                        Some(cursor) => {
                            buffer
                                .chunks
                                .iter()
                                .filter(|(sequence, _)| sequence >= cursor)
                                .map(|(_, bytes)| bytes.len())
                                .sum::<usize>()
                                >= BUFFER / 2
                        }
                        None => buffer.next > 0,
                    }
                };
                if !wait {
                    break;
                }
                notified.await;
            }
            let mut buffer = self.shared.lock();
            retain_preroll(&mut buffer, chunk);
            let sequence = buffer.next;
            buffer.next = buffer.next.checked_add(1).ok_or(Failure::Limit)?;
            buffer.bytes += chunk.len();
            buffer.chunks.push_back((sequence, Arc::from(chunk)));
            while buffer.bytes > BUFFER {
                let (_, removed) = buffer.chunks.pop_front().expect("nonempty buffer");
                buffer.bytes -= removed.len();
            }
            drop(buffer);
            self.shared.changed.notify_waiters();
            self.first.store(true, Ordering::Release);
            if let Some(ready) = self.ready.take() {
                let _ = ready.send(Some(Ok(())));
            }
        }
        self.pending.drain(..whole);
        Ok(())
    }
}

// Retain exact TS bytes beginning at the latest PAT packet. This bounded
// pre-roll preserves the latest PAT section's continuation and following media
// in their original order; a multi-section PAT may complete only after capture
// continues live. A new PAT atomically replaces the old interval, so changing
// programme versions/PIDs never injects obsolete cached PMT/media packets.
fn retain_preroll(buffer: &mut Buffer, bytes: &[u8]) {
    for packet in bytes.as_chunks::<188>().0 {
        let pid = (u16::from(packet[1] & 0x1f) << 8) | u16::from(packet[2]);
        if pid == 0 && packet[1] & 0xc0 == 0x40 && packet[3] & 0x10 != 0 {
            buffer.pre_roll.clear();
            buffer.pre_roll_valid = true;
            buffer.pre_roll_at = Some(tokio::time::Instant::now());
        }
        if buffer.pre_roll_valid {
            if buffer.pre_roll.len() + 188 > PRE_ROLL {
                buffer.pre_roll.clear();
                buffer.pre_roll_valid = false;
            } else {
                buffer.pre_roll.extend_from_slice(packet);
            }
        }
    }
}

/// A direct capture reader holds no player, surface or UI state.
pub struct LiveReader {
    shared: Arc<Shared>,
    cursor: u64,
    id: u64,
    prelude: Option<Arc<[u8]>>,
    owner: Option<Arc<Owner>>,
}
impl Drop for LiveReader {
    fn drop(&mut self) {
        self.shared.lock().readers.remove(&self.id);
        self.shared.changed.notify_waiters();
    }
}
impl LiveReader {
    fn new(shared: Arc<Shared>) -> Self {
        let (cursor, id, prelude) = {
            let mut b = shared.lock();
            let cursor = b.next;
            b.reader_id += 1;
            let id = b.reader_id;
            b.readers.insert(id, cursor);
            let prelude = (!b.pre_roll.is_empty()
                && b.pre_roll_at
                    .is_some_and(|at| at.elapsed() <= Duration::from_secs(2)))
            .then(|| Arc::from(b.pre_roll.as_slice()));
            (cursor, id, prelude)
        };
        shared.changed.notify_waiters();
        Self {
            shared,
            cursor,
            id,
            prelude,
            owner: None,
        }
    }
    pub async fn next(&mut self) -> Result<Arc<[u8]>, Failure> {
        if let Some(bytes) = self.prelude.take() {
            return Ok(bytes);
        }
        loop {
            let notified = self.shared.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut buffer = self.shared.lock();
                let first = buffer
                    .chunks
                    .front()
                    .map_or(buffer.next, |(sequence, _)| *sequence);
                if self.cursor < first {
                    return Err(Failure::Gap);
                }
                if let Some((_, bytes)) = buffer.chunks.get((self.cursor - first) as usize) {
                    let bytes = bytes.clone();
                    self.cursor += 1;
                    if let Some(cursor) = buffer.readers.get_mut(&self.id) {
                        *cursor = self.cursor;
                    }
                    drop(buffer);
                    self.shared.changed.notify_waiters();
                    return Ok(bytes);
                }
                if let Some(failure) = buffer.ended {
                    return Err(failure);
                }
            }
            notified.await;
        }
    }
}

async fn serve_connections<F, A>(mut accept: F, path: String, shared: Weak<Shared>)
where
    F: FnMut() -> A,
    A: std::future::Future<Output = io::Result<(tokio::net::TcpStream, std::net::SocketAddr)>>,
{
    let mut clients = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            Some(_) = clients.join_next(), if !clients.is_empty() => {},
            connection = accept(), if clients.len() < CLIENTS => {
                match connection {
                    Ok((socket, _)) => {
                        let shared = shared.clone();
                        let path = path.clone();
                        clients.spawn(async move { let _ = serve(socket, &path, shared).await; });
                    }
                    // Resource pressure must not retire a still-owned capability
                    // or its clients. Back off rather than spin on accept errors.
                    Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
                }
            }
        }
    }
}

async fn serve(
    mut socket: tokio::net::TcpStream,
    path: &str,
    shared: Weak<Shared>,
) -> io::Result<()> {
    let mut request = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let byte = socket.read_u8().await?;
            request.push(byte);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
            if request.len() > 4096 {
                return Err(io::Error::other("Request too long"));
            }
        }
        Ok(())
    })
    .await
    .map_err(io::Error::other)??;
    let request = std::str::from_utf8(&request).map_err(io::Error::other)?;
    let mut first = request
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    let method = first.next().unwrap_or_default();
    if !matches!(method, "GET" | "HEAD") || first.next() != Some(path) {
        socket
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await?;
        return Ok(());
    }
    let shared = shared
        .upgrade()
        .ok_or_else(|| io::Error::other("Input closed"))?;
    let mut reader = LiveReader::new(shared);
    socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n").await?;
    if method == "HEAD" {
        return Ok(());
    }
    loop {
        let bytes = reader
            .next()
            .await
            .map_err(|_| io::Error::other("Input ended or lagged"))?;
        tokio::time::timeout(Duration::from_secs(15), socket.write_all(&bytes))
            .await
            .map_err(io::Error::other)??;
    }
}

#[cfg(test)]
mod tests;

//! Capability-scoped, read-only local recording transport with byte ranges.
//! Native players continue to use their guarded HTTP path on every platform.
use std::{path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt},
    sync::oneshot,
};

pub struct RecordingFile {
    pub url: String,
    _stop: oneshot::Sender<()>,
    #[cfg(any(test, feature = "test-support"))]
    observation: WriteObserver,
}
impl RecordingFile {
    pub fn start(path: PathBuf) -> std::io::Result<Self> {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let mut random = [0u8; 24];
        getrandom::fill(&mut random).map_err(std::io::Error::other)?;
        let token: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let route = format!("/{token}/recording.ts");
        let url = format!("http://{address}{route}");
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let (stop, mut done) = oneshot::channel();
        let observer = WriteObserver::default();
        #[cfg(any(test, feature = "test-support"))]
        let observation = observer.clone();
        tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    _ = &mut done => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break };
                        // Finished tasks no longer consume an admission slot,
                        // even if accept wins the select before join_next.
                        while tasks.try_join_next().is_some() {}
                        if tasks.len() >= 4 { continue; }
                        let path = path.clone(); let route = route.clone();
                        let observer = observer.clone();
                        tasks.spawn(async move { let _ = serve(stream, path, &route, observer).await; });
                    },
                    _ = tasks.join_next(), if !tasks.is_empty() => {},
                }
            }
            tasks.abort_all();
        });
        Ok(Self {
            url,
            _stop: stop,
            #[cfg(any(test, feature = "test-support"))]
            observation,
        })
    }

    /// Authenticated response writes closed by this relay's idle deadline.
    /// Kept owner-scoped so native pause/resume tests cannot observe another
    /// player's transport or confuse request-header expiry with write stalls.
    #[cfg(any(test, feature = "test-support"))]
    pub fn write_timeouts(&self) -> usize {
        self.observation.count()
    }

    /// Successfully parsed authenticated GET byte-range requests for this relay.
    #[cfg(any(test, feature = "test-support"))]
    pub fn range_requests(&self) -> usize {
        self.observation.range_count()
    }
}
async fn serve(
    mut stream: tokio::net::TcpStream,
    path: PathBuf,
    route: &str,
    observer: WriteObserver,
) -> std::io::Result<()> {
    let mut head = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let b = stream.read_u8().await?;
            head.push(b);
            if head.ends_with(b"\r\n\r\n") {
                return Ok::<_, std::io::Error>(());
            }
            if head.len() >= 8192 {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
        }
    })
    .await??;
    let text = std::str::from_utf8(&head).map_err(std::io::Error::other)?;
    let mut lines = text.split("\r\n");
    let request: Vec<_> = lines
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    let mut output = ProgressWriter::new();
    if request.len() != 3 || !matches!(request[0], "GET" | "HEAD") || request[1] != route {
        output
            .send(
                &mut stream,
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await?;
        return Ok(());
    }
    // Unknown capabilities never contribute to a valid response's observer.
    output.observer = observer;
    let (file, metadata) = tokio::task::spawn_blocking(move || {
        crate::file::open_private_regular(&path, crate::file::PrivateAccess::Read)
    })
    .await
    .map_err(std::io::Error::other)??;
    let mut file = tokio::fs::File::from_std(file);
    let length = metadata.len();
    let range = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("range"))
        .map(|(_, value)| value.trim());
    let (start, end) = match byte_range(range, length) {
        Some(range) => range,
        None => {
            output.send(&mut stream, format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{length}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await?;
            return Ok(());
        }
    };
    if request[0] == "GET" && range.is_some() {
        output.observer.range_requested();
    }
    let status = if range.is_some() {
        "206 Partial Content"
    } else {
        "200 OK"
    };
    let content_range = if range.is_some() {
        format!("Content-Range: bytes {start}-{end}/{length}\r\n")
    } else {
        String::new()
    };
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: video/mp2t\r\nAccept-Ranges: bytes\r\n\
        {content_range}Content-Length: {}\r\nConnection: close\r\n\r\n",
        end - start + 1
    );
    output.send(&mut stream, header.as_bytes()).await?;
    if request[0] == "GET" {
        file.seek(std::io::SeekFrom::Start(start)).await?;
        let mut remaining = end - start + 1;
        let mut buffer = [0_u8; 64 * 1024];
        while remaining > 0 {
            let count = remaining.min(buffer.len() as u64) as usize;
            let read = file.read(&mut buffer[..count]).await?;
            if read == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            output.send(&mut stream, &buffer[..read]).await?;
            remaining -= read as u64;
        }
    }
    Ok(())
}
const WRITE_IDLE: Duration = Duration::from_secs(15);

#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
struct WriteCounts {
    timeouts: std::sync::atomic::AtomicUsize,
    ranges: std::sync::atomic::AtomicUsize,
}
#[derive(Clone, Default)]
struct WriteObserver {
    #[cfg(any(test, feature = "test-support"))]
    counts: std::sync::Arc<WriteCounts>,
}
impl WriteObserver {
    fn timed_out(&self) {
        #[cfg(any(test, feature = "test-support"))]
        self.counts
            .timeouts
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    #[cfg(any(test, feature = "test-support"))]
    fn count(&self) -> usize {
        self.counts
            .timeouts
            .load(std::sync::atomic::Ordering::Relaxed)
    }
    fn range_requested(&self) {
        #[cfg(any(test, feature = "test-support"))]
        self.counts
            .ranges
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    #[cfg(any(test, feature = "test-support"))]
    fn range_count(&self) -> usize {
        self.counts
            .ranges
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// One response's progress deadline spans headers and body chunks. A partial
/// positive write resets it; Pending/zero writes and file reads never do.
struct ProgressWriter {
    deadline: tokio::time::Instant,
    observer: WriteObserver,
}
impl ProgressWriter {
    fn new() -> Self {
        Self {
            deadline: tokio::time::Instant::now() + WRITE_IDLE,
            observer: WriteObserver::default(),
        }
    }
    async fn send(
        &mut self,
        writer: &mut (impl AsyncWrite + Unpin),
        mut bytes: &[u8],
    ) -> std::io::Result<()> {
        while !bytes.is_empty() {
            if tokio::time::Instant::now() >= self.deadline {
                self.observer.timed_out();
                return Err(std::io::ErrorKind::TimedOut.into());
            }
            let written = match tokio::time::timeout_at(self.deadline, writer.write(bytes)).await {
                Ok(result) => result?,
                Err(error) => {
                    self.observer.timed_out();
                    return Err(error.into());
                }
            };
            if written == 0 {
                return Err(std::io::ErrorKind::WriteZero.into());
            }
            self.deadline = tokio::time::Instant::now() + WRITE_IDLE;
            bytes = &bytes[written..];
        }
        Ok(())
    }
}

fn byte_range(value: Option<&str>, length: u64) -> Option<(u64, u64)> {
    if length == 0 {
        return None;
    }
    let Some(value) = value else {
        return Some((0, length - 1));
    };
    let (start, end) = value.strip_prefix("bytes=")?.split_once('-')?;
    let (start, end) = if start.is_empty() {
        let suffix: u64 = end.parse().ok()?;
        if suffix == 0 {
            return None;
        }
        (length.saturating_sub(suffix), length - 1)
    } else {
        (
            start.parse().ok()?,
            if end.is_empty() {
                length - 1
            } else {
                end.parse::<u64>().ok()?.min(length - 1)
            },
        )
    };
    (start <= end && start < length).then_some((start, end))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn seeking_ranges_are_bounded() {
        assert_eq!(byte_range(Some("bytes=20-"), 100), Some((20, 99)));
        assert_eq!(byte_range(Some("bytes=-20"), 100), Some((80, 99)));
        for range in ["bytes=100-", "bytes=30-20", "bytes=-0", "bytes=0-1,3-4"] {
            assert_eq!(byte_range(Some(range), 100), None);
        }
    }

    struct Trickle {
        next: std::pin::Pin<Box<tokio::time::Sleep>>,
        bytes: Vec<u8>,
    }
    impl AsyncWrite for Trickle {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            bytes: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            use std::future::Future;
            let this = self.get_mut();
            std::task::ready!(this.next.as_mut().poll(cx));
            this.next
                .as_mut()
                .reset(tokio::time::Instant::now() + Duration::from_secs(10));
            this.bytes.push(bytes[0]);
            std::task::Poll::Ready(Ok(1))
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test(start_paused = true)]
    async fn each_positive_partial_write_extends_the_idle_deadline() {
        let start = tokio::time::Instant::now();
        let mut writer = Trickle {
            next: Box::pin(tokio::time::sleep(Duration::from_secs(10))),
            bytes: Vec::new(),
        };
        ProgressWriter::new()
            .send(&mut writer, b"abc")
            .await
            .unwrap();
        assert_eq!(writer.bytes, b"abc");
        assert_eq!(tokio::time::Instant::now() - start, Duration::from_secs(30));
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_success_head_and_error_headers_share_the_write_deadline() {
        for header in [
            b"HTTP/1.1 200 OK\r\nContent-Length: 188\r\n\r\n".as_slice(),
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".as_slice(),
            b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\n\r\n".as_slice(),
        ] {
            let (mut writer, _reader) = tokio::io::duplex(1);
            let start = tokio::time::Instant::now();
            let error = ProgressWriter::new()
                .send(&mut writer, header)
                .await
                .unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
            assert_eq!(tokio::time::Instant::now() - start, WRITE_IDLE);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn moving_from_headers_to_body_does_not_reset_a_stalled_deadline() {
        let (mut writer, _reader) = tokio::io::duplex(1);
        let mut output = ProgressWriter::new();
        let start = tokio::time::Instant::now();
        output.send(&mut writer, b"h").await.unwrap();
        tokio::time::advance(Duration::from_secs(14)).await;
        let error = output.send(&mut writer, b"body").await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert_eq!(tokio::time::Instant::now() - start, WRITE_IDLE);
    }

    #[tokio::test(start_paused = true)]
    async fn write_timeout_observation_belongs_only_to_its_response_owner() {
        let owner = WriteObserver::default();
        let other = WriteObserver::default();
        let mut output = ProgressWriter::new();
        output.observer = owner.clone();
        let (mut writer, _reader) = tokio::io::duplex(1);
        let error = output
            .send(&mut writer, b"synthetic response")
            .await
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert_eq!(owner.count(), 1);
        assert_eq!(other.count(), 0);
        owner.range_requested();
        assert_eq!(owner.range_count(), 1);
        assert_eq!(other.range_count(), 0);
    }
}

//! Capability-scoped, read-only local recording transport with byte ranges.
//! Native players continue to use their guarded HTTP path on every platform.
use std::{path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::oneshot,
};

pub struct RecordingFile {
    pub url: String,
    _stop: oneshot::Sender<()>,
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
        tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    _ = &mut done => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break };
                        if tasks.len() >= 4 { continue; }
                        let path = path.clone(); let route = route.clone();
                        tasks.spawn(async move { let _ = serve(stream, path, &route).await; });
                    },
                    _ = tasks.join_next(), if !tasks.is_empty() => {},
                }
            }
            tasks.abort_all();
        });
        Ok(Self { url, _stop: stop })
    }
}
async fn serve(
    mut stream: tokio::net::TcpStream,
    path: PathBuf,
    route: &str,
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
    if request.len() != 3 || !matches!(request[0], "GET" | "HEAD") || request[1] != route {
        stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await?;
        return Ok(());
    }
    let meta = tokio::fs::symlink_metadata(&path).await?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(std::io::ErrorKind::PermissionDenied.into());
    }
    let mut file = tokio::fs::File::open(path).await?;
    let length = file.metadata().await?.len();
    let range = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("range"))
        .map(|(_, value)| value.trim());
    let (start, end) = match byte_range(range, length) {
        Some(range) => range,
        None => {
            stream.write_all(format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{length}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await?;
            return Ok(());
        }
    };
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
    stream.write_all(header.as_bytes()).await?;
    if request[0] == "GET" {
        file.seek(std::io::SeekFrom::Start(start)).await?;
        tokio::io::copy(&mut file.take(end - start + 1), &mut stream).await?;
    }
    Ok(())
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
}

//! Bounded MPEG-TS recorder. All media requests pass through the playback relay,
//! including manifest children: the recorder cannot bypass its header/DNS policy.
use crate::playback::{PlaybackPolicy, PlaybackRelay};
use std::io::Write;
use std::{path::Path, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Network,
    Unsupported,
    Storage,
    Limit,
    Gap,
}
impl Failure {
    pub fn message(self) -> &'static str {
        match self {
            Self::Network => "The recording stream ended or could not be read",
            Self::Unsupported => "Recording supports MPEG-TS and unencrypted MPEG-TS HLS streams",
            Self::Storage => "The recording could not be written; check storage space",
            Self::Limit => "The recording reached its storage limit",
            Self::Gap => "The stream skipped media segments; the recording is incomplete",
        }
    }
}

/// Dropping the capture future closes the file and relay, including a stalled fetch.
/// The caller owns stop/deadline selection and finalizes the acknowledged byte count.
pub async fn capture(
    url: &str,
    headers: &[(String, String)],
    policy: PlaybackPolicy,
    path: &Path,
    max_bytes: u64,
) -> Result<(), Failure> {
    let relay =
        PlaybackRelay::start_with_policy(url, headers, policy).map_err(|_| Failure::Network)?;
    let client = reqwest::Client::builder()
        .tls_backend_preconfigured(crate::tls::client_config(&[]).map_err(|_| Failure::Network)?)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| Failure::Network)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|_| Failure::Storage)?;
    let mut writer = Writer {
        file,
        bytes: 0,
        max_bytes,
    };
    let mut response = get(&client, &relay, relay.url()).await?;
    let mut head = Vec::new();
    while head.len() < 188 {
        let Some(chunk) = response.chunk().await.map_err(|_| Failure::Network)? else {
            break;
        };
        head.extend_from_slice(&chunk);
        if head.len() > 2 * 1024 * 1024 {
            return Err(Failure::Unsupported);
        }
    }
    if head.starts_with(b"#EXTM3U") {
        while let Some(chunk) = response.chunk().await.map_err(|_| Failure::Network)? {
            if head.len() + chunk.len() > 2 * 1024 * 1024 {
                return Err(Failure::Unsupported);
            }
            head.extend_from_slice(&chunk);
        }
        hls(&client, &relay, head, &mut writer).await?;
    } else {
        if head.first() != Some(&0x47) || head.len() < 188 {
            return Err(Failure::Unsupported);
        }
        writer.write(&head)?;
        while let Some(chunk) = response.chunk().await.map_err(|_| Failure::Network)? {
            writer.write(&chunk)?;
        }
    }
    writer.file.sync_all().map_err(|_| Failure::Storage)?;
    // Live input EOF before the requested deadline is never a successful full recording.
    Err(Failure::Network)
}

async fn get(
    client: &reqwest::Client,
    relay: &PlaybackRelay,
    url: &str,
) -> Result<reqwest::Response, Failure> {
    if !relay.owns_url(url) {
        return Err(Failure::Unsupported);
    }
    client
        .get(url)
        .send()
        .await
        .map_err(|_| Failure::Network)?
        .error_for_status()
        .map_err(|_| Failure::Network)
}
struct Writer {
    file: std::fs::File,
    bytes: u64,
    max_bytes: u64,
}
impl Writer {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Failure> {
        if self.bytes.saturating_add(bytes.len() as u64) > self.max_bytes {
            return Err(Failure::Limit);
        }
        self.file.write_all(bytes).map_err(|_| Failure::Storage)?;
        self.bytes += bytes.len() as u64;
        Ok(())
    }
}

struct Playlist {
    variant: Option<String>,
    segments: Vec<(u64, String)>,
    wait: Duration,
    ended: bool,
}
fn playlist(bytes: &[u8]) -> Result<Playlist, Failure> {
    let text = std::str::from_utf8(bytes).map_err(|_| Failure::Unsupported)?;
    if !text.starts_with("#EXTM3U") {
        return Err(Failure::Unsupported);
    }
    let mut p = Playlist {
        variant: None,
        segments: vec![],
        wait: Duration::from_secs(2),
        ended: false,
    };
    let mut sequence = 0;
    let mut variant = false;
    for line in text.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            sequence = value.parse().map_err(|_| Failure::Unsupported)?;
        } else if let Some(value) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
            let secs: u64 = value.parse().map_err(|_| Failure::Unsupported)?;
            p.wait = Duration::from_secs((secs / 2).clamp(1, 10));
        } else if line.starts_with("#EXT-X-STREAM-INF:") {
            variant = true;
        } else if line.starts_with("#EXT-X-MAP:")
            || line.starts_with("#EXT-X-BYTERANGE:")
            || line.starts_with("#EXT-X-MEDIA:")
            || line.starts_with("#EXT-X-SESSION-KEY:")
            || (line.starts_with("#EXT-X-KEY:") && line != "#EXT-X-KEY:METHOD=NONE")
        {
            return Err(Failure::Unsupported);
        } else if line == "#EXT-X-ENDLIST" {
            p.ended = true;
        } else if !line.is_empty() && !line.starts_with('#') {
            if variant {
                if p.variant.is_none() {
                    p.variant = Some(line.into());
                }
                variant = false;
            } else {
                p.segments.push((sequence, line.into()));
                sequence = sequence.checked_add(1).ok_or(Failure::Unsupported)?;
            }
        }
    }
    Ok(p)
}
async fn manifest(
    client: &reqwest::Client,
    relay: &PlaybackRelay,
    url: &str,
) -> Result<Vec<u8>, Failure> {
    let mut response = get(client, relay, url).await?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Failure::Network)? {
        if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
            return Err(Failure::Unsupported);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
async fn hls(
    client: &reqwest::Client,
    relay: &PlaybackRelay,
    mut bytes: Vec<u8>,
    writer: &mut Writer,
) -> Result<(), Failure> {
    let mut url = relay.url().to_owned();
    // Bound nested master playlists and never accumulate a segment-URI history.
    for _ in 0..4 {
        let p = playlist(&bytes)?;
        let Some(variant) = p.variant else { break };
        url = variant;
        bytes = manifest(client, relay, &url).await?;
    }
    let mut next = None;
    let mut last_progress = tokio::time::Instant::now();
    loop {
        let p = playlist(&bytes)?;
        if p.variant.is_some() || p.segments.is_empty() {
            return Err(Failure::Unsupported);
        }
        // Start near the live edge, rather than silently recording the entire rewind window.
        let first = if next.is_none() && !p.ended {
            p.segments.last().unwrap().0
        } else {
            p.segments[0].0
        };
        let expected = *next.get_or_insert(first);
        if p.segments[0].0 > expected {
            return Err(Failure::Gap);
        }
        for (sequence, segment) in p.segments {
            if sequence < next.unwrap() {
                continue;
            }
            if sequence != next.unwrap() {
                return Err(Failure::Gap);
            }
            // Buffer one bounded segment, so cancellation cannot leave half a segment in the file.
            let mut response = get(client, relay, &segment).await?;
            let mut data = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| Failure::Network)? {
                if data.len() + chunk.len() > 32 * 1024 * 1024 {
                    return Err(Failure::Limit);
                }
                data.extend_from_slice(&chunk);
            }
            if data.len() < 188
                || data.len() % 188 != 0
                || !data
                    .as_chunks::<188>()
                    .0
                    .iter()
                    .all(|packet| packet[0] == 0x47)
            {
                return Err(Failure::Unsupported);
            }
            writer.write(&data)?;
            last_progress = tokio::time::Instant::now();
            next = sequence.checked_add(1);
            if next.is_none() {
                return Err(Failure::Unsupported);
            }
        }
        if p.ended {
            return Err(Failure::Network);
        }
        if last_progress.elapsed() > Duration::from_secs(30) {
            return Err(Failure::Network);
        }
        tokio::time::sleep(p.wait).await;
        bytes = manifest(client, relay, &url).await?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sequences_and_unsupported_formats_are_explicit() {
        let p = playlist(b"#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:24\n#EXTINF:2,\nhttps://example.test/a.ts\n#EXTINF:2,\nhttps://example.test/b.ts\n").unwrap();
        assert_eq!(p.segments[1].0, 25);
        for tag in [
            "#EXT-X-KEY:METHOD=AES-128,URI=\"key\"",
            "#EXT-X-MAP:URI=\"init.mp4\"",
            "#EXT-X-BYTERANGE:188@0",
        ] {
            assert!(matches!(
                playlist(format!("#EXTM3U\n{tag}\na.ts\n").as_bytes()),
                Err(Failure::Unsupported)
            ));
        }
    }
}

//! Provider artwork crosses the same DNS/redirect boundary as other content.
//! NSH receives bytes as a data URI, never an untrusted address to fetch itself.
use base64::Engine;
use futures_channel::oneshot;
use nsh_core::prelude::*;
use std::{
    collections::VecDeque,
    sync::{Mutex, MutexGuard, OnceLock, PoisonError},
    time::Duration,
};
use tokio::sync::Semaphore;

const IMAGE_BYTES: u64 = 4 << 20;
const CACHE_BYTES: usize = 8 << 20;
type Reply = oneshot::Sender<String>;
#[derive(Default)]
struct Cache {
    entries: VecDeque<(String, String)>,
    bytes: usize,
}
impl Cache {
    fn get(&mut self, url: &str) -> Option<String> {
        if let Some(at) = self.entries.iter().position(|(key, _)| key == url) {
            let (key, value) = self.entries.remove(at)?;
            self.entries.push_back((key, value.clone()));
            Some(value)
        } else {
            None
        }
    }
    /// Stores `value` for `url` as the newest entry. An address already
    /// held is replaced, not stored twice (two views asking for the same
    /// logo both miss, then both insert): a duplicate would be counted
    /// against the cache's bytes while `get` only ever finds the first.
    fn insert(&mut self, url: String, value: String) {
        if let Some(at) = self.entries.iter().position(|(key, _)| *key == url)
            && let Some((key, old)) = self.entries.remove(at)
        {
            self.bytes -= key.len() + old.len();
        }
        let size = url.len() + value.len();
        while self.bytes + size > CACHE_BYTES || self.entries.len() >= 64 {
            let Some((key, value)) = self.entries.pop_front() else {
                break;
            };
            self.bytes -= key.len() + value.len();
        }
        self.bytes += size;
        self.entries.push_back((url, value));
    }
}
fn cache() -> MutexGuard<'static, Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    // A poisoned lock is taken all the same: the UI thread reads this
    // cache while rendering and must not panic because a download thread
    // did. Nothing can panic between `Cache`'s own updates of `entries`
    // and `bytes` (queue operations and integer arithmetic on what the
    // queue holds), so a panic elsewhere under the lock leaves them
    // consistent.
    locked(CACHE.get_or_init(|| Mutex::new(Cache::default())))
}
fn locked(cache: &Mutex<Cache>) -> MutexGuard<'_, Cache> {
    cache.lock().unwrap_or_else(PoisonError::into_inner)
}
/// Downloads at once at most; more wait their turn (cheaply: a pending
/// future each), and one whose view moved on gives up its turn.
const PARALLEL: usize = 4;

fn permits() -> &'static Semaphore {
    static PERMITS: Semaphore = Semaphore::const_new(PARALLEL);
    &PERMITS
}

/// One client for every artwork download, made on the network runtime.
fn client() -> Option<&'static iptv_net::NetClient> {
    static CLIENT: OnceLock<Option<iptv_net::NetClient>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            let config = iptv_net::NetConfig {
                max_streamed_bytes: IMAGE_BYTES,
                max_buffered_bytes: IMAGE_BYTES as usize,
                request_timeout: Duration::from_secs(10),
                allow_files: false,
                #[cfg(debug_assertions)]
                allow_private_fixture: cfg!(feature = "test-support")
                    || crate::data::DEV_PLAYLIST.is_some(),
                ..Default::default()
            };
            iptv_net::NetClient::new(config).ok()
        })
        .as_ref()
}

/// The artwork at `url` as a data URI (the no-logo image when it cannot
/// be had), from the cache or downloaded on the shared runtime. `None`
/// when `wanted` was dropped (its view shows something else) before its
/// turn came.
async fn fetch(url: String, wanted: Reply) -> Option<String> {
    let _turn = permits().acquire().await.ok()?;
    if wanted.is_canceled() {
        return None;
    }
    if let Some(value) = cache().get(&url) {
        return Some(value);
    }
    let value = match client() {
        Some(net) => download(net, &url).await,
        None => None,
    }
    .unwrap_or_else(crate::grid::no_logo);
    cache().insert(url, value.clone());
    let _ = wanted.send(value.clone());
    Some(value)
}

async fn download(net: &iptv_net::NetClient, url: &str) -> Option<String> {
    let mut request = iptv_providers::HttpRequest::get(url);
    request.public_only = true;
    let response = net.fetch(request).await.ok()?;
    if !response.is_success() {
        return None;
    }
    // All native SVG paths use nsh_core::svg: its resolver resolves neither
    // external URLs/files nor embedded image hrefs. Data URIs preserve SVG.
    Some(format!(
        "data:image/octet-stream;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(response.body)
    ))
}
/// A recycled view changes its URL reactively; a late response must never
/// overwrite the new channel's artwork. Queueing and network work are bounded.
pub(crate) fn source(url: impl Fn() -> String + 'static) -> impl Fn() -> String {
    let shown = signal(crate::grid::no_logo());
    let generation = signal(0u64);
    effect(move || {
        let url = url();
        let current = generation.get_untracked().wrapping_add(1);
        generation.set(current);
        let cached = cache().get(&url);
        if let Some(value) = cached {
            shown.set(value);
            return;
        }
        shown.set(crate::grid::no_logo());
        if url.len() > 16 * 1024 || !(url.starts_with("http://") || url.starts_with("https://")) {
            return;
        }
        let (tx, rx) = oneshot::channel();
        // Detached: the download fills the cache even if this view moves
        // on meanwhile; dropping `rx` (the effect's next run ends this
        // task) only skips a download that has not started.
        drop(crate::executor::spawn(fetch(url, tx)));
        drop(spawn(async move {
            if let Ok(value) = rx.await
                && generation.get_untracked() == current
            {
                shown.set(value);
            }
        }));
    });
    move || shown.get()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_has_byte_and_entry_bounds() {
        let mut c = Cache::default();
        for n in 0..100 {
            c.insert(n.to_string(), "x".repeat(200_000));
        }
        assert!(c.bytes <= CACHE_BYTES);
        assert!(c.entries.len() <= 64);
        assert!(c.get("99").is_some());
        assert!(c.get("0").is_none());
    }
    #[test]
    fn an_address_is_held_once_and_a_repeat_makes_it_the_newest() {
        let mut c = Cache::default();
        c.insert("a".into(), "first".into());
        c.insert("b".into(), "other".into());
        c.insert("a".into(), "second, longer".into());
        assert_eq!(c.entries.len(), 2);
        assert_eq!(c.get("a").as_deref(), Some("second, longer"));
        assert_eq!(
            c.bytes,
            "a".len() + "second, longer".len() + "b".len() + "other".len()
        );
        // "a" was refreshed, so "b" is now the oldest and goes first.
        for n in 0..62 {
            c.insert(format!("more-{n}"), "x".into());
        }
        assert_eq!(c.entries.len(), 64);
        c.insert("one-more".into(), "x".into());
        assert!(c.get("b").is_none());
        assert!(c.get("a").is_some());
        let held: usize = c.entries.iter().map(|(k, v)| k.len() + v.len()).sum();
        assert_eq!(c.bytes, held);
    }
    #[test]
    fn cache_eviction_is_least_recently_used() {
        let mut c = Cache::default();
        c.insert("a".into(), "1".into());
        c.insert("b".into(), "2".into());
        // Access "a" so it is promoted to most recently used.
        assert_eq!(c.get("a").as_deref(), Some("1"));
        // Fill up to 64 entries.
        for n in 0..62 {
            c.insert(format!("more-{n}"), "x".into());
        }
        assert_eq!(c.entries.len(), 64);
        // Next insertion must evict "b" (least recently used), not "a".
        c.insert("overflow".into(), "x".into());
        assert!(c.get("b").is_none(), "b should be evicted as LRU");
        assert!(c.get("a").is_some(), "a was accessed so it is kept");
    }
    #[test]
    fn a_poisoned_cache_lock_still_serves() {
        use std::sync::Arc;
        let cache = Arc::new(Mutex::new(Cache::default()));
        locked(&cache).insert("logo".into(), "kept".into());
        let held = cache.clone();
        let poisoner = std::thread::spawn(move || {
            let _guard = locked(&held);
            // Unwinds like a panic, without printing one.
            std::panic::resume_unwind(Box::new("synthetic"));
        });
        assert!(poisoner.join().is_err());
        assert!(cache.is_poisoned());
        assert_eq!(locked(&cache).get("logo").as_deref(), Some("kept"));
        locked(&cache).insert("next".into(), "stored".into());
        assert_eq!(locked(&cache).entries.len(), 2);
    }
    #[test]
    fn private_and_file_images_are_refused_before_connection() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let net = iptv_net::NetClient::new(Default::default()).unwrap();
            for url in [
                "file:///etc/hosts",
                "http://127.0.0.1:9/logo",
                "http://[::1]:9/logo",
            ] {
                assert!(download(&net, url).await.is_none());
            }
        });
    }
    #[test]
    fn fetched_png_and_svg_reach_native_decoder_as_identical_bytes() {
        use std::io::{Read, Write};
        let png=base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jZfkAAAAASUVORK5CYII=").unwrap();
        let svg=br#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><rect width="2" height="2" fill="red"/></svg>"#.to_vec();
        let expected = [png, svg];
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let bodies = expected.clone();
        let server = std::thread::spawn(move || {
            for body in bodies {
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = [0; 4096];
                let _ = socket.read(&mut request).unwrap();
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                socket.write_all(&body).unwrap();
            }
        });
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let config = iptv_net::NetConfig {
                allow_private_fixture: true,
                ..Default::default()
            };
            let net = iptv_net::NetClient::new(config).unwrap();
            for (index, bytes) in expected.iter().enumerate() {
                let uri = download(&net, &format!("http://{address}/{index}"))
                    .await
                    .unwrap();
                assert!(uri.starts_with("data:"));
                let (_, decoded) = nsh_core::image::decode_data_uri(&uri).unwrap();
                assert_eq!(&decoded, bytes);
                if index == 1 {
                    assert!(nsh_core::svg::sniff(&decoded));
                    assert!(nsh_core::svg::rasterize(&decoded, 2).is_ok());
                } else {
                    assert!(decoded.starts_with(b"\x89PNG\r\n\x1a\n"));
                }
            }
        });
        server.join().unwrap();
    }
}

//! Web application layer of iptv-rs, for TV web runtimes (Samsung Tizen).
//!
//! The portable core runs in WebAssembly inside a Web Worker. The page's
//! JavaScript (`hosts/tizen`) renders the views this emits, sends remote
//! keys and plays streams with the TV's native player; the worker's
//! JavaScript performs the HTTP requests the import describes.
//!
//! - `host`: the imports the module calls (fetch, chunks, timers, storage,
//!   events, clocks).
//! - `chunks`, `files`, `device`, `package`: for an application on the
//!   real runtime, the browser store's chunks and the app's small files in
//!   IndexedDB, randomness, the time zone and the package's own files.
//! - `exec`: a single-threaded executor polled after every delivery, and
//!   the `iptv_import::Fetch` adapter over the host.
//! - `store`: what is kept between launches: the stored guide (an
//!   `iptv_epg` snapshot) with its refresh state, and the user's lists.
//! - `app`: Live TV: import a source (Xtream or M3U), its guide, the
//!   guide screen under the preview, full screen and zapping, moving
//!   favorites, on `iptv_core`'s list, guide focus and reorder models.
//! - `grid`: the guide screen's rail, groups column and grid, on
//!   `iptv_core`'s guide focus and browse columns, emitting only the rows
//!   and time on screen.
//! - `search`: channel and program search with `iptv_search`'s indexes and
//!   search screen state.
//!
//! Live channels only: movies and series are not imported here.
//!
//! **Destination policy.** Content-supplied addresses (guide feeds named by
//! a playlist, stream and logo addresses) must be public, by
//! `iptv_providers::content_url`; redirects are checked again and may not
//! downgrade HTTPS. The browser resolves host names itself and does not
//! expose the answer, so a public name that resolves to a LAN address is not
//! detected, and the TV's image loader and player follow their own
//! redirects.
//!
//! The exports are a C ABI: bytes cross through [`input`] (a buffer the host
//! fills before an export that reads it) and the host's `emit` import.

#[cfg(feature = "bridge")]
mod app;
mod chunks;
mod device;
mod exec;
mod files;
#[cfg(feature = "bridge")]
mod grid;
mod host;
mod package;
#[cfg(feature = "bridge")]
mod presentation;
#[cfg(feature = "bridge")]
mod search;
#[cfg(feature = "bridge")]
mod store;

/// The adapters an application on the real runtime builds its ports from
/// (`iptv-app`'s web host): the host's fetch, its timer and clock, and the
/// executor polled after every delivery.
pub mod adapters {
    use std::future::Future;
    use std::time::Duration;

    pub use crate::exec::{HostBody, HostFetch};

    /// A browser store's chunks in the host's IndexedDB.
    pub mod chunks {
        pub use crate::chunks::{Unreadable, read_stored, release_stored, save, stored_size};
    }

    /// The app's small files in the host's IndexedDB.
    pub mod files {
        pub use crate::files::StoredFiles;
    }

    pub use crate::device::{fill_random, utc_offset_minutes};
    pub use crate::package::package_file;

    /// Runs `task` from the executor's next poll on; outside a poll, one is
    /// asked of the host.
    pub fn spawn(task: impl Future<Output = ()> + 'static) {
        crate::exec::spawn(task);
    }

    /// Polls the executor until no task can progress. iptv-web's own exports
    /// poll after every delivery; a host that delivers through another
    /// door (an NSH event) calls this after it.
    pub fn poll() {
        crate::exec::poll();
    }

    /// Ends after `duration`, on the host's timer.
    pub async fn sleep(duration: Duration) {
        crate::exec::sleep_while(duration, || true).await;
    }

    /// The host's monotonic clock.
    pub fn monotonic() -> Duration {
        Duration::from_secs_f64((crate::host::host().monotonic_ms() / 1000.0).max(0.0))
    }

    /// The value stored under `key` in the host's key-value storage.
    pub async fn storage_get(key: &str) -> Option<Vec<u8>> {
        crate::exec::storage_get(key).await
    }

    /// Stores `value` under `key`; writes keep their order. Empty deletes.
    pub fn storage_put(key: &str, value: &[u8]) {
        crate::exec::storage_put(key, value);
    }

    /// The host's wall clock: time since the Unix epoch.
    pub fn wall() -> Duration {
        Duration::from_secs_f64((crate::host::host().wall_ms() / 1000.0).max(0.0))
    }
}

#[cfg(all(test, feature = "bridge"))]
mod tests;

use std::cell::RefCell;

thread_local! {
    static INPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn take_input(len: u32) -> Vec<u8> {
    INPUT.with(|i| {
        let mut i = i.borrow_mut();
        i.truncate(len as usize);
        std::mem::take(&mut *i)
    })
}

/// A buffer of `len` bytes the host writes the next export's input into.
#[unsafe(no_mangle)]
pub extern "C" fn input(len: u32) -> *mut u8 {
    INPUT.with(|i| {
        let mut i = i.borrow_mut();
        i.clear();
        i.resize(len as usize, 0);
        i.as_mut_ptr()
    })
}

/// A command (JSON, `len` bytes in [`input`]).
#[cfg(feature = "bridge")]
#[unsafe(no_mangle)]
pub extern "C" fn command(len: u32) {
    let bytes = take_input(len);
    app::command(&String::from_utf8_lossy(&bytes));
    exec::poll();
}

/// Request `id` answered with `status`; `length` is the declared body size,
/// negative when unknown.
#[unsafe(no_mangle)]
pub extern "C" fn on_status(id: u32, status: u32, length: f64) {
    let length = (length >= 0.0).then_some(length as u64);
    exec::deliver_status(id, status.min(u32::from(u16::MAX)) as u16, length);
    exec::poll();
}

/// Request `id` was redirected to the URL (`len` bytes in [`input`]); sent
/// before its status. A refused hop fails the request.
#[unsafe(no_mangle)]
pub extern "C" fn on_redirect(id: u32, len: u32) {
    let url = String::from_utf8_lossy(&take_input(len)).into_owned();
    exec::deliver_redirect(id, &url);
    exec::poll();
}

/// The whole body of request `id` (`len` bytes in [`input`]).
#[unsafe(no_mangle)]
pub extern "C" fn on_body(id: u32, len: u32) {
    exec::deliver_body(id, take_input(len));
    exec::poll();
}

/// The next chunk of stream `id` (`len` bytes in [`input`]).
#[unsafe(no_mangle)]
pub extern "C" fn on_chunk(id: u32, len: u32) {
    exec::deliver_chunk(id, Some(take_input(len)));
    exec::poll();
}

/// Stream `id` ended.
#[unsafe(no_mangle)]
pub extern "C" fn on_end(id: u32) {
    exec::deliver_chunk(id, None);
    exec::poll();
}

/// Request `id` failed: `kind` 1 connect, 2 idle timeout, 3 body, 4 invalid
/// request, 5 too large, 7 the import's deadline, else other; the detail
/// (`len` bytes) is in [`input`].
#[unsafe(no_mangle)]
pub extern "C" fn on_error(id: u32, kind: u32, len: u32) {
    let detail = String::from_utf8_lossy(&take_input(len)).into_owned();
    exec::deliver_error(id, kind, detail);
    exec::poll();
}

/// The stored value asked for with `id` (`len` bytes in [`input`]).
#[unsafe(no_mangle)]
pub extern "C" fn on_stored(id: u32, len: u32) {
    exec::deliver_stored(id, Some(take_input(len)));
    exec::poll();
}

/// Nothing is stored under the key asked for with `id`.
#[unsafe(no_mangle)]
pub extern "C" fn on_missing(id: u32) {
    exec::deliver_stored(id, None);
    exec::poll();
}

/// Chunk or file save `id` committed (`ok` 1) or failed (`chunks::save`,
/// `files::save`).
#[unsafe(no_mangle)]
pub extern "C" fn on_chunks_saved(id: u32, ok: u32) {
    exec::deliver_saved(id, ok == 1);
    exec::poll();
}

/// The timer `id` fired.
#[unsafe(no_mangle)]
pub extern "C" fn on_wake(id: u32) {
    exec::deliver_wake(id);
    exec::poll();
}

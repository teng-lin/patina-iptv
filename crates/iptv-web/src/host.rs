//! The JavaScript host's side of the boundary.
//!
//! Rust calls the host through [`Host`]; the host answers by calling the
//! module's exports (`lib.rs`), never from inside one of these calls, and then
//! [`crate::exec::poll`]. In WebAssembly the host is the module's `host`
//! imports; tests install a recording fake.

use std::cell::RefCell;
use std::rc::Rc;

/// What the core asks of its host. Every request is answered later and
/// asynchronously through the exports.
pub(crate) trait Host {
    /// Starts an HTTP request. `request` is JSON: `url`, `method`,
    /// `headers` (`[[name, value]]`), `body` (bytes) and `timeoutMs`. A full
    /// request ends with `on_status` then `on_body`; a stream ends its
    /// status with `on_status` and its body is read with [`Host::fetch_next`].
    fn fetch(&self, id: u32, request: &str, stream: bool);
    /// Reads the next chunk of stream `id` (`on_chunk` or `on_end`).
    fn fetch_next(&self, id: u32);
    /// The body of `id` is no longer wanted.
    fn fetch_close(&self, id: u32);
    /// Calls `on_wake(id)` after `ms` milliseconds.
    fn sleep(&self, id: u32, ms: f64);
    /// Reads `key` from the app's storage: `on_stored(id, len)` with the
    /// value in the input buffer, or `on_missing(id)`.
    fn store_get(&self, id: u32, key: &str);
    /// Writes `value` under `key`, in call order; an empty value deletes it.
    fn store_put(&self, key: &str, value: &[u8]);
    /// One JSON event for the UI: a view, a request to play, progress.
    #[cfg_attr(not(feature = "bridge"), allow(dead_code))]
    fn emit(&self, event: &str);
    /// A monotonic clock in milliseconds.
    fn monotonic_ms(&self) -> f64;
    /// Wall clock, Unix milliseconds.
    fn wall_ms(&self) -> f64;
}

thread_local! {
    static HOST: RefCell<Option<Rc<dyn Host>>> = const { RefCell::new(None) };
}

#[cfg(all(test, feature = "bridge"))]
pub(crate) fn install(host: Rc<dyn Host>) {
    HOST.with(|h| *h.borrow_mut() = Some(host));
}

/// The host, unless the thread is shutting down (cleanup from destructors
/// of tasks dropped with the thread must not touch destroyed state).
pub(crate) fn host_if_alive() -> Option<Rc<dyn Host>> {
    HOST.try_with(|h| h.borrow().clone()).ok().flatten()
}

pub(crate) fn host() -> Rc<dyn Host> {
    HOST.with(|h| {
        if h.borrow().is_none() {
            *h.borrow_mut() = Some(default_host());
        }
        h.borrow().clone().expect("host installed")
    })
}

#[cfg(target_arch = "wasm32")]
fn default_host() -> Rc<dyn Host> {
    Rc::new(wasm::WasmHost)
}

#[cfg(not(target_arch = "wasm32"))]
fn default_host() -> Rc<dyn Host> {
    panic!("no host installed: native builds install one (tests do)")
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    #[link(wasm_import_module = "host")]
    unsafe extern "C" {
        fn fetch(id: u32, ptr: *const u8, len: usize, stream: u32);
        fn fetch_next(id: u32);
        fn fetch_close(id: u32);
        fn sleep(id: u32, ms: f64);
        fn store_get(id: u32, key: *const u8, key_len: usize);
        fn store_put(key: *const u8, key_len: usize, value: *const u8, value_len: usize);
        #[cfg_attr(not(feature = "bridge"), allow(dead_code))]
        fn emit(ptr: *const u8, len: usize);
        fn monotonic_ms() -> f64;
        fn wall_ms() -> f64;
    }

    pub(crate) struct WasmHost;

    impl super::Host for WasmHost {
        fn fetch(&self, id: u32, request: &str, stream: bool) {
            // SAFETY: the host reads `len` bytes at `ptr` before returning and
            // keeps no reference; `request` outlives the call.
            unsafe { fetch(id, request.as_ptr(), request.len(), u32::from(stream)) }
        }
        fn fetch_next(&self, id: u32) {
            // SAFETY: plain value arguments.
            unsafe { fetch_next(id) }
        }
        fn fetch_close(&self, id: u32) {
            // SAFETY: plain value arguments.
            unsafe { fetch_close(id) }
        }
        fn sleep(&self, id: u32, ms: f64) {
            // SAFETY: plain value arguments.
            unsafe { sleep(id, ms) }
        }
        fn store_get(&self, id: u32, key: &str) {
            // SAFETY: the host copies `key_len` bytes at `key` before returning.
            unsafe { store_get(id, key.as_ptr(), key.len()) }
        }
        fn store_put(&self, key: &str, value: &[u8]) {
            // SAFETY: the host copies both byte ranges before returning.
            unsafe { store_put(key.as_ptr(), key.len(), value.as_ptr(), value.len()) }
        }
        fn emit(&self, event: &str) {
            // SAFETY: the host copies `len` bytes at `ptr` before returning.
            unsafe { emit(event.as_ptr(), event.len()) }
        }
        fn monotonic_ms(&self) -> f64 {
            // SAFETY: no arguments; returns a number.
            unsafe { monotonic_ms() }
        }
        fn wall_ms(&self) -> f64 {
            // SAFETY: no arguments; returns a number.
            unsafe { wall_ms() }
        }
    }
}

//! What the browser build of this executable needs from its runner
//! (`tools/wasm-test.mjs`): the host services SQLite links against, a
//! panic report the runner can print (the target has no stdout), and a
//! stand-in for `tempfile`.
//!
//! The runner implements the imports of module `env`: `test_report`
//! (UTF-8 text), `test_now_ms` and `test_random`. 64-bit values do not cross
//! the boundary.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn test_report(text: *const u8, len: usize);
    fn test_now_ms() -> f64;
    fn test_random(buf: *mut u8, len: usize);
}

/// Prints `text` through the runner (the target has no stdout).
#[allow(dead_code)]
pub fn report(text: &str) {
    // SAFETY: the runner reads `len` bytes at `text`, which stay borrowed.
    unsafe { test_report(text.as_ptr(), text.len()) }
}

/// Milliseconds of the runner's clock, for measurements.
#[allow(dead_code)]
pub fn now_ms() -> f64 {
    // SAFETY: no arguments.
    unsafe { test_now_ms() }
}

/// Called by the runner before `main`: panics abort the run, so their
/// message is printed first.
#[unsafe(no_mangle)]
pub extern "C" fn test_install_panic_hook() {
    std::panic::set_hook(Box::new(|info| report(&info.to_string())));
}

#[unsafe(export_name = "rust_sqlite_wasm_host_sleep")]
pub extern "C" fn host_sleep(_seconds: u64, _nanoseconds: u32) {}

/// # Safety
/// `buf` is writable for `len` bytes.
#[unsafe(export_name = "rust_sqlite_wasm_host_random")]
pub unsafe extern "C" fn host_random(buf: *mut u8, len: usize) -> usize {
    // SAFETY: the caller's contract; the runner fills exactly `len` bytes.
    unsafe { test_random(buf, len) };
    len
}

/// # Safety
/// `out` is writable.
#[unsafe(export_name = "rust_sqlite_wasm_host_epoch_timestamp_in_ms")]
pub unsafe extern "C" fn host_epoch_ms(out: *mut i64) -> i32 {
    // SAFETY: the caller's contract; `test_now_ms` takes no arguments.
    unsafe { out.write(test_now_ms() as i64) };
    0
}

/// # Safety
/// `buf` is writable for `len` bytes.
#[unsafe(export_name = "rust_sqlite_wasm_host_fill_entropy")]
pub unsafe extern "C" fn host_fill_entropy(buf: *mut u8, len: usize) -> i32 {
    // SAFETY: as for `host_random`.
    unsafe { test_random(buf, len) };
    0
}

/// Local time is not needed by the store (it keeps Unix times); SQLite's
/// `localtime` modifier reports an error.
#[unsafe(export_name = "rust_sqlite_wasm_host_localtime")]
pub extern "C" fn host_localtime(_unix_seconds: i64, _out: *mut u8) -> i32 {
    1
}

/// A name prefix no other test uses, in place of a temporary directory.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn path(&self) -> &Path {
        &self.0
    }
}

pub fn tempdir() -> std::io::Result<TempDir> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    Ok(TempDir(PathBuf::from(format!(
        "/tmp/test-{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))))
}

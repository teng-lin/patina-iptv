//! Complete native SQLCipher initialization before a connection is opened.
//!
//! Bundled SQLite publishes its core-ready flag before its SQLCipher extra
//! hook finishes. Another first-opener may return from `sqlite3_initialize`
//! while the cipher provider is absent. Run the idempotent cipher hook after
//! core initialization, then wait for its export registration to be visible.
//! Only successful process readiness is cached; no key, path or owner is kept.
use rusqlite::{Connection, Result, ffi};
use std::{
    sync::OnceLock,
    time::{Duration, Instant},
};

// Exact ABI of sqlcipher-src 420.0.0 / SQLCipher 4.20.0, pinned in Cargo.lock.
// Its export auto-extension callback is static, so it cannot be registered here.
unsafe extern "C" {
    fn sqlcipher_extra_init(argument: *const std::ffi::c_char) -> std::ffi::c_int;
}

static READY: OnceLock<()> = OnceLock::new();
const EXPORT_WAIT: Duration = Duration::from_secs(5);

/// Finish native initialization, including the SQLCipher export auto-extension.
/// Errors are returned to the caller and never cached or treated as plaintext.
pub fn initialize() -> Result<()> {
    if READY.get().is_some() {
        return Ok(());
    }
    // SAFETY: SQLite documents this API as thread-safe and idempotent. This
    // adapter never installs shutdown callbacks or changes threading mode.
    check(unsafe { ffi::sqlite3_initialize() })?;
    // SAFETY: the pinned hook accepts a null argument and serializes itself
    // with SQLite's static master mutex. Core initialization already returned,
    // so its recursive SQLite allocations cannot re-enter incomplete core init.
    // It either waits for a foreign first-opener or completes cipher setup here.
    check(unsafe { sqlcipher_extra_init(std::ptr::null()) })?;

    wait_for_export(&READY, export_registered, Instant::now() + EXPORT_WAIT)
}

fn export_registered() -> Result<bool> {
    // The hook registers the extension after releasing its mutex. Each probe
    // opens a fresh connection so it can see an extension registered later.
    let connection = Connection::open_in_memory()?;
    let query = "SELECT EXISTS(SELECT 1 FROM pragma_function_list WHERE name='sqlcipher_export')";
    connection.query_row(query, [], |row| row.get(0))
}

fn wait_for_export(
    ready: &OnceLock<()>,
    mut registered: impl FnMut() -> Result<bool>,
    deadline: Instant,
) -> Result<()> {
    loop {
        if registered()? {
            let _ = ready.set(());
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(rusqlite::Error::SqliteFailure(
                ffi::Error::new(ffi::SQLITE_BUSY),
                Some("SQLCipher export initialization did not complete".into()),
            ));
        }
        std::thread::yield_now();
    }
}

fn check(code: std::ffi::c_int) -> Result<()> {
    if code == ffi::SQLITE_OK {
        Ok(())
    } else {
        Err(rusqlite::Error::SqliteFailure(ffi::Error::new(code), None))
    }
}

#[cfg(test)]
mod tests;

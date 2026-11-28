//! The browser store: SQLite on `iptv-store-sqlite`'s memory files,
//! restored from IndexedDB at launch and saved there in 64 KiB chunks after
//! every commit (`docs/plans/2026-11-21-browser-tv-architecture.md`,
//! decision 2). The host page loads the stored chunks before the module
//! starts (`hosts/tizen-nsh`), so the open, which cannot wait, reads them
//! synchronously.
//!
//! A save is one commit's take of changed chunks, written in one IndexedDB
//! transaction and then acknowledged ([`ChangedChunks::saved`]); commits
//! made while it is written are taken by the next save, which the runtime
//! runs on its writer once this one ended. A failed save offers its chunks
//! again after [`RETRY`]. Storage therefore always holds one whole commit:
//! a crash can lose the last seconds, never mix two commits.
//!
//! Also here: the five host hooks `sqlite-wasm-rs` links against
//! (`rust_sqlite_wasm_host_*`), over the host's clock, randomness and
//! time zone. Their 64-bit values stay inside the module; the host
//! imports behind them take and return `f64` and 32-bit integers
//! (Chromium 69 has no 64-bit integers at the JavaScript boundary).

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use iptv_store_sqlite::{BrowserFiles, CHUNK_BYTES, Store, StoreError};
use iptv_web::adapters::chunks;

use iptv_runtime::store_host::{Persist, StoreHost};

/// How long a failed save waits before its chunks are offered again (a
/// full quota or a busy database does not clear at once).
const RETRY: Duration = Duration::from_secs(5);

/// The store's memory files and the state of their saves.
#[derive(Default)]
pub(crate) struct BrowserStore {
    saves: Arc<Mutex<Saves>>,
}

#[derive(Default)]
struct Saves {
    /// A save is being written.
    writing: bool,
    /// A commit arrived while it was: save again once it ended.
    behind: Option<Persist>,
    /// The database's size in the last save that committed.
    saved_size: Option<u64>,
}

fn lock(saves: &Mutex<Saves>) -> std::sync::MutexGuard<'_, Saves> {
    saves.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Restores the stored database into `files`: its size, `None` when none
/// is stored.
fn restore(files: &BrowserFiles) -> Result<Option<u64>, StoreError> {
    let size = chunks::stored_size().map_err(|_| {
        // Opening over a database the host could not read would replace
        // it with an empty one at the first save.
        StoreError::Unavailable("the TV's storage could not be read")
    })?;
    let Some(size) = size else {
        return Ok(None);
    };
    let token = files.restore_size(size)?;
    let mut buffer = vec![0; CHUNK_BYTES];
    let count = size.div_ceil(CHUNK_BYTES as u64);
    for index in 0..count {
        let start = index * CHUNK_BYTES as u64;
        let len = (size - start).min(CHUNK_BYTES as u64) as usize;
        let index = index as u32;
        if !chunks::read_stored(index, &mut buffer[..len]) {
            return Err(StoreError::InvalidData(format!(
                "stored chunk {index} is missing"
            )));
        }
        files.restore_chunk(token, index, &buffer[..len])?;
    }
    Ok(Some(size))
}

impl StoreHost for BrowserStore {
    fn open(&self) -> Result<Store, StoreError> {
        let started = iptv_web::adapters::monotonic();
        let files = BrowserFiles::new()?;
        let restored = restore(&files)?;
        let store = Store::open_browser(&files)?;
        // The store holds its own copy now.
        chunks::release_stored();
        lock(&self.saves).saved_size = restored;
        log::info!(
            "iptv store: opened the browser store ({}) in {} ms",
            match restored {
                Some(size) => format!("{size} bytes restored"),
                None => "new".to_owned(),
            },
            (iptv_web::adapters::monotonic() - started).as_millis()
        );
        Ok(store)
    }

    fn committed(&self, store: &Store, again: Persist) {
        let mut saves = lock(&self.saves);
        if saves.writing {
            saves.behind = Some(again);
            return;
        }
        let take = match store.take_changed_chunks() {
            Ok(take) => take,
            // A transaction is open (a job that failed half way): the
            // commit that ends it saves.
            Err(error) => {
                log::debug!("iptv store: nothing to save yet: {error}");
                return;
            }
        };
        let size = take.size();
        if take.is_empty() && saves.saved_size == Some(size) {
            take.saved();
            return;
        }
        saves.writing = true;
        drop(saves);
        let started = iptv_web::adapters::monotonic();
        let written = chunks::save(take.chunks(), size);
        let count = take.len();
        let state = self.saves.clone();
        iptv_web::adapters::spawn(async move {
            let ok = written.await;
            if ok {
                take.saved();
                lock(&state).saved_size = Some(size);
                log::info!(
                    "iptv store: saved {count} chunks in {} ms",
                    (iptv_web::adapters::monotonic() - started).as_millis()
                );
            } else {
                // Return the chunks to the next take. Keep the writer
                // reserved during the delay so commits join the retry.
                drop(take);
                log::warn!("iptv store: the TV did not save {count} chunks; retrying");
                iptv_web::adapters::sleep(RETRY).await;
            }
            let next = {
                let mut saves = lock(&state);
                saves.writing = false;
                saves.behind.take().or_else(|| (!ok).then_some(again))
            };
            if let Some(next) = next {
                next();
            }
        });
    }

    fn begin_publish(&self, store: &mut Store) -> Result<Store, StoreError> {
        store.begin_publish()
    }

    fn finish_publish(&self, store: &mut Store, copy: Store) -> Result<(), StoreError> {
        store.finish_publish(copy)
    }

    fn abandon_publish(&self, store: &mut Store, copy: Store) {
        if let Err(error) = store.abandon_publish(copy) {
            log::warn!("iptv store: abandoning a publish: {error}");
        }
    }
}

// The host services SQLite links against (`sqlite_wasm_rs::host`).

/// SQLite sleeps only in a busy handler, which the browser store never
/// sets: there is nothing to wait for on one thread.
#[unsafe(export_name = "rust_sqlite_wasm_host_sleep")]
pub extern "C" fn host_sleep(_seconds: u64, _nanoseconds: u32) {}

/// # Safety
///
/// `buf` is writable for `len` bytes (SQLite's contract).
#[unsafe(export_name = "rust_sqlite_wasm_host_random")]
pub unsafe extern "C" fn host_random(buf: *mut u8, len: usize) -> usize {
    // SAFETY: the caller's contract.
    let buf = unsafe { std::slice::from_raw_parts_mut(buf, len) };
    if iptv_web::adapters::fill_random(buf).is_ok() {
        len
    } else {
        0
    }
}

/// # Safety
///
/// `out` is writable (SQLite's contract).
#[unsafe(export_name = "rust_sqlite_wasm_host_epoch_timestamp_in_ms")]
pub unsafe extern "C" fn host_epoch_ms(out: *mut i64) -> i32 {
    let ms = iptv_web::adapters::wall().as_millis();
    // SAFETY: the caller's contract.
    unsafe { out.write(i64::try_from(ms).unwrap_or(i64::MAX)) };
    0
}

/// # Safety
///
/// `buf` is writable for `len` bytes (SQLite's contract).
#[unsafe(export_name = "rust_sqlite_wasm_host_fill_entropy")]
pub unsafe extern "C" fn host_fill_entropy(buf: *mut u8, len: usize) -> i32 {
    // SAFETY: the caller's contract.
    let buf = unsafe { std::slice::from_raw_parts_mut(buf, len) };
    // 2: the service is unavailable (`sqlite_wasm_rs::host::Error`).
    if iptv_web::adapters::fill_random(buf).is_ok() {
        0
    } else {
        2
    }
}

/// Local calendar fields as `sqlite_wasm_rs::host::LocalTime` lays them out.
#[repr(C)]
pub struct LocalTime {
    year: i32,
    month: i32,
    day: i32,
    hour: i32,
    minute: i32,
    second: i32,
    weekday: i32,
    yearday: i32,
    is_dst: i32,
    utc_offset_seconds: i32,
}

/// # Safety
///
/// `out` is writable (SQLite's contract).
#[unsafe(export_name = "rust_sqlite_wasm_host_localtime")]
pub unsafe extern "C" fn host_localtime(unix_seconds: i64, out: *mut LocalTime) -> i32 {
    let offset = i64::from(iptv_web::adapters::utc_offset_minutes(unix_seconds)) * 60;
    let Some(local) = unix_seconds.checked_add(offset) else {
        return 3; // InvalidTime
    };
    let days = local.div_euclid(86_400);
    let second_of_day = local.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let Ok(year32) = i32::try_from(year) else {
        return 3;
    };
    let yearday = days - days_from_civil(year, 1, 1);
    let time = LocalTime {
        year: year32,
        month: month as i32,
        day: day as i32,
        hour: (second_of_day / 3600) as i32,
        minute: (second_of_day / 60 % 60) as i32,
        second: (second_of_day % 60) as i32,
        // 1970-01-01 was a Thursday (4).
        weekday: (days + 4).rem_euclid(7) as i32,
        yearday: yearday as i32,
        is_dst: -1,
        utc_offset_seconds: offset as i32,
    };
    // SAFETY: the caller's contract.
    unsafe { out.write(time) };
    0
}

/// The proleptic Gregorian date of `days` since 1970-01-01 (Howard
/// Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let month = i64::from(month);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

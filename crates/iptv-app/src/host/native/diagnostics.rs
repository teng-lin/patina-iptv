//! Native platform sinks, panic persistence (`last-panic.txt`), non-blocking
//! 1 MiB × 3 desktop log rotation (`patina.log`), and save/share delivery.

use std::cell::Cell;
use std::collections::{BTreeSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Once, OnceLock, PoisonError, RwLock, TryLockError};
use std::thread::{JoinHandle, ThreadId};
use std::time::{Duration, Instant, SystemTime};

use log::Log as _;

use crate::host::diagnostics::{DiagnosticLogger, ExportOutcome, FileSink};

/// Rotate `patina.log` at 1 MiB, keeping `.1` and `.2` (at most ~3 MiB total).
pub(crate) const ROTATE_BYTES: u64 = 1024 * 1024;
const ROTATE_RETRY_STEP_BYTES: u64 = 64 * 1024;
const DIAGNOSTICS_SUBDIR: &str = "diagnostics";
const LOG_FILE_NAME: &str = "patina.log";
const LAST_PANIC_FILE_NAME: &str = "last-panic.txt";
const LAST_PANIC_REPORTED_NAME: &str = ".last-panic.reported";
const SHARE_PREFIX: &str = "patina-diagnostics-";
const SHARE_RETENTION: Duration = Duration::from_secs(24 * 3600);
const MAX_RETAINED_SHARES: usize = 64;
const MAX_LAST_PANIC_READ_BYTES: u64 = 64 * 1024;
const WRITER_QUEUE_CAPACITY: usize = 1_024;
#[cfg(any(target_os = "macos", target_os = "ios"))]
const APPLE_OS_LOG_ENTRY_BYTES: usize = 960;

static ORIGIN: OnceLock<Instant> = OnceLock::new();
static PANIC_HOOK: Once = Once::new();
static PANIC_DIR: RwLock<Option<PathBuf>> = RwLock::new(None);
static ACTIVATED: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);
static ENV_LOGGER: OnceLock<Option<env_logger::Logger>> = OnceLock::new();

thread_local! {
    static IN_PANIC: Cell<bool> = const { Cell::new(false) };
}

/// Android's `TvActivity` loads `libnsh.so` and runs `nsh.runRustApp("nsh")`
/// directly (without JS `DevSession`). Registering `DiagnosticLogger` in
/// `.init_array` runs just before `JNI_OnLoad` -> `logger::init()` with two
/// lock-free atomic stores and zero heap allocation, ensuring every `Info`+
/// record is both buffered and forwarded to `__android_log_write("nsh", ...)`.
#[cfg(target_os = "android")]
unsafe extern "C" fn android_early_init() {
    crate::host::diagnostics::install_logger_early();
}

#[cfg(target_os = "android")]
#[used]
// SAFETY: `.init_array` expects a valid `extern "C" fn()` pointer; `android_early_init`
// performs only lock-free atomic stores on static state without touching TLS or I/O.
#[unsafe(link_section = ".init_array")]
static ANDROID_INIT: unsafe extern "C" fn() = android_early_init;

pub(crate) fn elapsed() -> Duration {
    ORIGIN.get_or_init(Instant::now).elapsed()
}

fn platform_max_level() -> log::LevelFilter {
    if let Some(logger) = env_sink() {
        return logger.filter().max(log::LevelFilter::Info);
    }
    log::LevelFilter::Info
}

fn env_sink() -> Option<&'static env_logger::Logger> {
    ENV_LOGGER
        .get_or_init(|| {
            // Windows release builds run under `windows_subsystem = "windows"` with
            // no stderr handle; keep `env_logger` stderr output for debug builds on
            // Windows and for all Linux/macOS runs.
            let enabled = cfg!(any(
                target_os = "linux",
                target_os = "macos",
                all(windows, debug_assertions),
            ));
            enabled.then(|| {
                let env = env_logger::Env::default().default_filter_or("info");
                env_logger::Builder::from_env(env).build()
            })
        })
        .as_ref()
}

pub(crate) fn platform_enabled(metadata: &log::Metadata<'_>) -> bool {
    if let Some(logger) = env_sink() {
        return logger.enabled(metadata);
    }
    metadata.level() <= log::Level::Info
}

pub(crate) fn platform_log(record: &log::Record<'_>) {
    if let Some(logger) = env_sink() {
        if logger.matches(record) {
            logger.log(record);
        }
        // Packaged macOS apps launched from Finder/Dock have stderr on `/dev/null`;
        // mirror warnings and errors into Unified Logging so Console.app sees them.
        #[cfg(target_os = "macos")]
        if record.level() <= log::Level::Warn {
            apple_os_log(record.level(), record.target(), &record.args().to_string());
        }
    } else if record.level() <= log::Level::Info {
        #[cfg(target_os = "android")]
        android_log_write(record.level(), record.target(), &record.args().to_string());
        #[cfg(target_os = "ios")]
        {
            let msg = record.args().to_string();
            apple_os_log(record.level(), record.target(), &msg);
            if cfg!(debug_assertions) || std::env::var_os("NSH_DEVELOPMENT").is_some() {
                let _ = writeln!(
                    io::stderr().lock(),
                    "{:>5} {}: {msg}",
                    record.level(),
                    record.target()
                );
            }
        }
    }
}

pub(crate) fn platform_flush() {
    if let Some(logger) = env_sink() {
        logger.flush();
    }
}

/// Splits `message` into UTF-8 chunks of at most `max` bytes (preferring the
/// last newline in range), matching `nsh-platform-apple`'s Unified Logging limit.
#[cfg(any(target_os = "macos", target_os = "ios", test))]
pub(crate) fn os_log_parts(message: &str, max: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = Some(message);
    while let Some(text) = rest {
        if text.len() <= max {
            out.push(text);
            break;
        }
        let end = text
            .floor_char_boundary(max)
            .max(text.ceil_char_boundary(1));
        let (part, next) = match text[..end].rfind('\n').filter(|&nl| nl > 0) {
            Some(nl) => (&text[..nl], &text[nl + 1..]),
            None => text.split_at(end),
        };
        out.push(part);
        rest = (!next.is_empty()).then_some(next);
    }
    out
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn apple_os_log(level: log::Level, target: &str, message: &str) {
    use std::ffi::{CString, c_char};

    unsafe extern "C" {
        fn patina_apple_os_log(
            level: u8,
            subsystem: *const c_char,
            category: *const c_char,
            message: *const c_char,
        );
    }

    let sanitize_c = |s: &str| {
        let escaped = if s.contains('\0') {
            s.replace('\0', "\\0")
        } else {
            s.to_owned()
        };
        CString::new(escaped).unwrap_or_default()
    };
    let category = sanitize_c(if target.is_empty() { "iptv" } else { target });
    let prefix = if level == log::Level::Warn {
        "WARN "
    } else {
        ""
    };
    for part in os_log_parts(message, APPLE_OS_LOG_ENTRY_BYTES) {
        let body = format!("{prefix}{part}");
        let text = sanitize_c(&body);
        // SAFETY: all pointers reference valid NUL-terminated C strings during the call.
        unsafe {
            patina_apple_os_log(
                level as u8,
                c"com.hantekllc.iptvrs".as_ptr(),
                category.as_ptr(),
                text.as_ptr(),
            );
        }
    }
}

#[cfg(target_os = "android")]
fn android_log_write(level: log::Level, target: &str, message: &str) {
    use std::ffi::{CString, c_char, c_int};

    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }

    let priority = match level {
        log::Level::Error => 6,
        log::Level::Warn => 5,
        log::Level::Info => 4,
        log::Level::Debug => 3,
        log::Level::Trace => 2,
    };
    let clean = format!("{target}: {message}").replace('\0', " ");
    let text = CString::new(clean).unwrap_or_default();
    // SAFETY: `c"nsh"` and `text` are valid NUL-terminated C strings for the call duration.
    unsafe {
        __android_log_write(priority, c"nsh".as_ptr(), text.as_ptr());
    }
}

pub(crate) fn install(logger: &'static DiagnosticLogger) {
    let _ = ORIGIN.get_or_init(Instant::now);
    let max_level = platform_max_level();
    seed_env_roots(logger, None);
    let _ = log::set_logger(logger);
    if log::max_level() < max_level {
        log::set_max_level(max_level);
    }
    install_panic_hook(logger);
}

fn seed_env_roots(logger: &DiagnosticLogger, extra: Option<&Path>) {
    let mut roots = Vec::new();
    if let Some(dir) = extra {
        roots.push(dir.to_string_lossy().into_owned());
        if let Ok(canon) = dir.canonicalize() {
            roots.push(canon.to_string_lossy().into_owned());
        }
    }
    for var in ["HOME", "LOCALAPPDATA", "USERPROFILE", "APPDATA"] {
        if let Some(val) = std::env::var_os(var) {
            let path = PathBuf::from(val);
            if path.is_absolute() {
                roots.push(path.to_string_lossy().into_owned());
            }
        }
    }
    logger.set_roots(roots);
}

pub(crate) fn seed_roots(logger: &DiagnosticLogger, directory: &Path) {
    seed_env_roots(logger, Some(directory));
}

pub(crate) fn set_panic_dir(logger: &DiagnosticLogger, directory: &Path) {
    seed_env_roots(logger, Some(directory));
    *PANIC_DIR.write().unwrap_or_else(PoisonError::into_inner) = Some(directory.to_path_buf());
}

/// Returns only an explicitly owned storage directory (`set_panic_dir`): never
/// resolves unowned `HOME` paths so a rejected second instance or early exit
/// cannot touch another instance's storage or block on test/env locks.
fn try_panic_dir() -> Option<PathBuf> {
    match PANIC_DIR.try_read() {
        Ok(guard) => guard.clone(),
        Err(TryLockError::Poisoned(err)) => err.into_inner().clone(),
        Err(TryLockError::WouldBlock) => None,
    }
}

fn install_panic_hook(logger: &'static DiagnosticLogger) {
    PANIC_HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if iptv_runtime::execution::recovering_worker_spawn(info.payload()) {
                return;
            }
            struct Guard;
            impl Drop for Guard {
                fn drop(&mut self) {
                    let _ = IN_PANIC.try_with(|in_panic| in_panic.set(false));
                }
            }
            let already_panicking = IN_PANIC
                .try_with(|in_panic| in_panic.replace(true))
                .unwrap_or(true);
            if !already_panicking {
                let _guard = Guard;
                record_panic(logger, info);
            }
            previous(info);
        }));
    });
}

fn record_panic(logger: &'static DiagnosticLogger, info: &std::panic::PanicHookInfo<'_>) {
    let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = info.payload().downcast_ref::<String>() {
        s.clone()
    } else {
        "non_string_panic".to_owned()
    };
    let current = std::thread::current();
    let thread_name = current.name().unwrap_or("unnamed");
    let location = info
        .location()
        .map(|loc| (loc.file(), loc.line(), loc.column()));
    let backtrace =
        backtrace_enabled().then(|| std::backtrace::Backtrace::force_capture().to_string());
    let dir = try_panic_dir();
    let mut roots = logger.try_roots_snapshot();
    if let Some(dir) = &dir {
        let extra =
            crate::host::diagnostics::redact::normalize_roots([dir.to_string_lossy().into_owned()]);
        for item in extra {
            if !roots.contains(&item) {
                roots.push(item);
            }
        }
        roots.sort_by_key(|s| std::cmp::Reverse(s.len()));
    }
    let (summary, report) = crate::host::diagnostics::buffer::format_panic_report(
        env!("CARGO_PKG_VERSION"),
        env!("IPTV_GIT_HASH"),
        include_str!("../../../../../nsh.rev").trim(),
        crate::host::diagnostics::platform_label(super::PLATFORM),
        env!("IPTV_TARGET"),
        elapsed(),
        thread_name,
        location,
        &payload,
        backtrace.as_deref(),
        &roots,
    );
    logger.record_panic_nonblocking(&summary, &roots);
    if let Some(dir) = dir {
        let _ = write_last_panic_atomic(&dir, report.as_bytes());
    }
}

fn backtrace_enabled() -> bool {
    std::env::var_os("RUST_BACKTRACE").is_some_and(|v| !v.is_empty() && v != "0")
}

fn is_reparse_or_symlink(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

/// Creates `<data_dir>/diagnostics` inside an already-existing `data_dir` without
/// ever recreating a missing or relocated `data_dir` (e.g. during backup restore).
pub(crate) fn ensure_diagnostics_dir(data_dir: &Path) -> io::Result<PathBuf> {
    let parent_meta = fs::symlink_metadata(data_dir)?;
    if is_reparse_or_symlink(&parent_meta) || !parent_meta.is_dir() {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    let dir = data_dir.join(DIAGNOSTICS_SUBDIR);
    if let Ok(meta) = fs::symlink_metadata(&dir)
        && is_reparse_or_symlink(&meta)
    {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(&dir) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
        Err(err) => return Err(err),
    }
    let meta = fs::symlink_metadata(&dir)?;
    if is_reparse_or_symlink(&meta) || !meta.is_dir() {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    super::platform_data::exclude_from_backup(&dir)?;
    Ok(dir)
}

fn open_private_read(path: &Path) -> io::Result<File> {
    let meta = fs::symlink_metadata(path)?;
    if is_reparse_or_symlink(&meta) || !meta.file_type().is_file() {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    opts.open(path)
}

fn create_private_file(path: &Path, append: bool) -> io::Result<File> {
    if let Ok(meta) = fs::symlink_metadata(path)
        && is_reparse_or_symlink(&meta)
    {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    let mut opts = OpenOptions::new();
    opts.create(true).write(true);
    if append {
        opts.append(true);
    } else {
        opts.truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = opts.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
    }
    Ok(file)
}

fn create_new_temp_file(path: &Path) -> io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    opts.open(path)
}

pub(crate) fn write_last_panic_atomic(data_dir: &Path, bytes: &[u8]) -> io::Result<()> {
    let diag_dir = ensure_diagnostics_dir(data_dir)?;
    let pid = std::process::id();
    let mut created = None;
    let mut last_err = None;
    for _ in 0..8 {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let cand = diag_dir.join(format!(".{LAST_PANIC_FILE_NAME}.tmp.{pid}.{seq}"));
        match create_new_temp_file(&cand) {
            Ok(file) => {
                created = Some((cand, file));
                break;
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                last_err = Some(err);
            }
            Err(err) => return Err(err),
        }
    }
    let Some((tmp, mut file)) = created else {
        return Err(last_err.unwrap_or_else(|| io::Error::from(io::ErrorKind::AlreadyExists)));
    };
    let dest = diag_dir.join(LAST_PANIC_FILE_NAME);
    let reported = diag_dir.join(LAST_PANIC_REPORTED_NAME);
    let write_res = (|| -> io::Result<()> {
        file.write_all(bytes)?;
        file.flush()?;
        let _ = file.sync_all();
        drop(file);
        fs::rename(&tmp, &dest)?;
        let _ = fs::remove_file(&reported);
        Ok(())
    })();
    if write_res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    write_res
}

pub(crate) fn read_last_panic_in(data_dir: &Path) -> Option<String> {
    let path = data_dir.join(DIAGNOSTICS_SUBDIR).join(LAST_PANIC_FILE_NAME);
    let file = open_private_read(&path).ok()?;
    let mut buf = Vec::new();
    file.take(MAX_LAST_PANIC_READ_BYTES)
        .read_to_end(&mut buf)
        .ok()?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    (!text.trim().is_empty()).then_some(text)
}

pub(crate) fn read_last_panic() -> Option<String> {
    let dir = try_panic_dir()?;
    read_last_panic_in(&dir)
}

fn should_report_previous_panic(data_dir: &Path, panic_text: &str) -> bool {
    let marker = data_dir
        .join(DIAGNOSTICS_SUBDIR)
        .join(LAST_PANIC_REPORTED_NAME);
    let stamp = format!(
        "len={}\n{}",
        panic_text.len(),
        panic_text.lines().next().unwrap_or("")
    );
    if let Ok(mut file) = open_private_read(&marker) {
        let mut existing = String::new();
        if file.read_to_string(&mut existing).is_ok() && existing == stamp {
            return false;
        }
    }
    if let Ok(mut file) = create_private_file(&marker, false) {
        let _ = file.write_all(stamp.as_bytes());
        let _ = file.flush();
    }
    true
}

/// Prunes staged share attachments older than `SHARE_RETENTION` or beyond the
/// `MAX_RETAINED_SHARES` newest files, preserving recent URIs so asynchronous
/// Android/iOS share targets can still open an attachment after a second share.
fn prune_expired_share_files(diag_dir: &Path) {
    let Ok(entries) = fs::read_dir(diag_dir) else {
        return;
    };
    let now = SystemTime::now();
    let mut kept: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if !name_str.starts_with(SHARE_PREFIX) || !name_str.ends_with(".txt") {
            continue;
        }
        let path = entry.path();
        let modified = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let expired = now
            .duration_since(modified)
            .is_ok_and(|age| age > SHARE_RETENTION);
        if expired {
            let _ = fs::remove_file(path);
        } else {
            kept.push((modified, path));
        }
    }
    if kept.len() > MAX_RETAINED_SHARES {
        kept.sort_by_key(|(mtime, _)| *mtime);
        let excess = kept.len() - MAX_RETAINED_SHARES;
        for (_, old_path) in kept.into_iter().take(excess) {
            let _ = fs::remove_file(old_path);
        }
    }
}

pub(crate) fn activate_storage(logger: &'static DiagnosticLogger, directory: &Path) {
    set_panic_dir(logger, directory);
    let first_time = ACTIVATED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(directory.to_path_buf());
    if !first_time {
        return;
    }
    if let Ok(diag_dir) = ensure_diagnostics_dir(directory) {
        prune_expired_share_files(&diag_dir);
    }
    if let Some(previous_panic) = read_last_panic_in(directory)
        && should_report_previous_panic(directory, &previous_panic)
    {
        let roots = logger.roots_snapshot();
        let brief = summarize_previous_panic(&previous_panic, &roots);
        log::warn!("iptv diagnostics: previous run ended in a panic ({brief})");
    }
    let desktop =
        cfg!(any(target_os = "linux", target_os = "macos", windows)) && !crate::host::is_tui_mode();
    if desktop
        && let Ok(diag_dir) = ensure_diagnostics_dir(directory)
        && let Ok(writer) = RotatingWriter::spawn(diag_dir, ROTATE_BYTES)
    {
        logger.attach_file_sink(Arc::new(writer));
    }
}

pub(crate) fn shutdown(logger: &'static DiagnosticLogger) {
    logger.close_file_sink();
    ACTIVATED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}

pub(crate) fn summarize_previous_panic(text: &str, roots: &[String]) -> String {
    use std::fmt::Write as _;

    let mut thread = "unknown";
    let mut location = "unknown";
    for line in text.lines() {
        if let Some(val) = line.strip_prefix("thread=") {
            thread = val.trim();
        } else if let Some(val) = line.strip_prefix("location=") {
            location = val.trim();
        }
    }
    let mut raw = String::new();
    let _ = write!(raw, "thread={thread}");
    raw.push(' ');
    let _ = write!(raw, "location={location}");
    crate::host::diagnostics::redact::redact_text(&raw, roots)
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

fn rename_if_exists(from: &Path, to: &Path) -> io::Result<()> {
    if !from.exists() {
        return Ok(());
    }
    fs::rename(from, to)
}

/// Synchronous rotating file core (`patina.log`, `.1`, `.2`), driven off-thread
/// by [`RotatingWriter`] in production and directly in deterministic unit tests.
pub(crate) struct RotatingLog {
    dir: PathBuf,
    max_bytes: u64,
    next_rotate_at: u64,
    writer: Option<BufWriter<File>>,
    written: u64,
}

impl RotatingLog {
    pub(crate) fn open(dir: PathBuf, max_bytes: u64) -> io::Result<Self> {
        let active = dir.join(LOG_FILE_NAME);
        if let Ok(meta) = fs::symlink_metadata(&active)
            && is_reparse_or_symlink(&meta)
        {
            let _ = fs::remove_file(&active);
        }
        let effective_max = max_bytes.max(64);
        let file = create_private_file(&active, true)?;
        let written = file.metadata().map_or(0, |m| m.len());
        Ok(Self {
            dir,
            max_bytes: effective_max,
            next_rotate_at: effective_max,
            writer: Some(BufWriter::with_capacity(8 * 1024, file)),
            written,
        })
    }

    pub(crate) fn write_line(&mut self, line: &str) -> io::Result<()> {
        let bytes = line.as_bytes();
        if self.writer.is_none() {
            let active = self.dir.join(LOG_FILE_NAME);
            let file = create_private_file(&active, true)?;
            self.written = file.metadata().map_or(0, |m| m.len());
            self.writer = Some(BufWriter::with_capacity(8 * 1024, file));
        }
        if self.written > 0
            && self.written.saturating_add(bytes.len() as u64) > self.next_rotate_at
            && let Err(err) = self.rotate()
        {
            // Keep existing `patina.log` intact in append mode if Windows or OS
            // permissions temporarily deny renaming the active log, bounded at
            // `2 * max_bytes` so a locked file never grows without bound.
            let active = self.dir.join(LOG_FILE_NAME);
            if self.writer.is_none()
                && let Ok(reopened) = create_private_file(&active, true)
            {
                self.written = reopened.metadata().map_or(self.written, |m| m.len());
                self.writer = Some(BufWriter::with_capacity(8 * 1024, reopened));
            }
            let retry_step = ROTATE_RETRY_STEP_BYTES.min(self.max_bytes);
            self.next_rotate_at = self.written.saturating_add(retry_step);
            if self.writer.is_none() {
                return Err(err);
            }
        }
        let hard_cap = self.max_bytes.saturating_mul(2);
        if self.written > 0 && self.written.saturating_add(bytes.len() as u64) > hard_cap {
            return Ok(());
        }
        let Some(writer) = self.writer.as_mut() else {
            return Ok(());
        };
        writer.write_all(bytes)?;
        self.written = self.written.saturating_add(bytes.len() as u64);
        Ok(())
    }

    pub(crate) fn flush(&mut self) -> io::Result<()> {
        if let Some(writer) = self.writer.as_mut() {
            writer.flush()?;
        }
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        // Close the active handle before renaming so Windows allows moving `patina.log`.
        if let Some(mut writer) = self.writer.take() {
            let _ = writer.flush();
        }
        let active = self.dir.join(LOG_FILE_NAME);
        let staged = self.dir.join(format!("{LOG_FILE_NAME}.rotating"));
        let first = self.dir.join(format!("{LOG_FILE_NAME}.1"));
        let second = self.dir.join(format!("{LOG_FILE_NAME}.2"));
        // Secure the active file into `.rotating` BEFORE touching `.2` or `.1` so
        // a locked `patina.log` on Windows leaves rotated history untouched.
        remove_if_exists(&staged)?;
        rename_if_exists(&active, &staged)?;
        let roll_res = (|| -> io::Result<()> {
            remove_if_exists(&second)?;
            rename_if_exists(&first, &second)?;
            rename_if_exists(&staged, &first)?;
            Ok(())
        })();
        if let Err(err) = roll_res {
            if !active.exists() && staged.exists() {
                let _ = fs::rename(&staged, &active);
            }
            return Err(err);
        }
        let fresh = create_private_file(&active, false)?;
        self.writer = Some(BufWriter::with_capacity(8 * 1024, fresh));
        self.written = 0;
        self.next_rotate_at = self.max_bytes;
        Ok(())
    }
}

struct WriterQueue {
    seed: Vec<String>,
    lines: VecDeque<String>,
    dropped: u64,
    submitted_epoch: u64,
    flushed_epoch: u64,
    flush_requested: bool,
    closed: bool,
}

struct SharedQueue {
    state: Mutex<WriterQueue>,
    wake: Condvar,
    capacity: usize,
}

pub(crate) struct RotatingWriter {
    shared: Arc<SharedQueue>,
    worker_thread: ThreadId,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for RotatingWriter {
    fn drop(&mut self) {
        self.signal_close();
    }
}

impl RotatingWriter {
    pub(crate) fn spawn(dir: PathBuf, max_bytes: u64) -> io::Result<Self> {
        Self::spawn_with_capacity(dir, max_bytes, WRITER_QUEUE_CAPACITY)
    }

    pub(crate) fn spawn_with_capacity(
        dir: PathBuf,
        max_bytes: u64,
        capacity: usize,
    ) -> io::Result<Self> {
        let mut log = RotatingLog::open(dir, max_bytes)?;
        let shared = Arc::new(SharedQueue {
            state: Mutex::new(WriterQueue {
                seed: Vec::new(),
                lines: VecDeque::new(),
                dropped: 0,
                submitted_epoch: 0,
                flushed_epoch: 0,
                flush_requested: false,
                closed: false,
            }),
            wake: Condvar::new(),
            capacity: capacity.max(1),
        });
        let worker_shared = shared.clone();
        let handle = std::thread::Builder::new()
            .name("iptv-diag-log".into())
            .spawn(move || Self::worker_loop(&mut log, &worker_shared))?;
        let worker_thread = handle.thread().id();
        Ok(Self {
            shared,
            worker_thread,
            worker: Mutex::new(Some(handle)),
        })
    }

    fn signal_close(&self) {
        let mut q = self
            .shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        q.closed = true;
        q.flush_requested = true;
        self.shared.wake.notify_all();
    }

    fn worker_loop(log: &mut RotatingLog, shared: &SharedQueue) {
        loop {
            let (seed, batch, missed, epoch, closing) = {
                let mut q = shared.state.lock().unwrap_or_else(PoisonError::into_inner);
                while q.seed.is_empty()
                    && q.lines.is_empty()
                    && q.dropped == 0
                    && !q.flush_requested
                    && !q.closed
                {
                    q = shared.wake.wait(q).unwrap_or_else(PoisonError::into_inner);
                }
                if q.seed.is_empty()
                    && q.lines.is_empty()
                    && q.dropped == 0
                    && !q.flush_requested
                    && q.closed
                {
                    break;
                }
                let seed = std::mem::take(&mut q.seed);
                let batch = std::mem::take(&mut q.lines);
                let missed = std::mem::replace(&mut q.dropped, 0);
                q.flush_requested = false;
                (seed, batch, missed, q.submitted_epoch, q.closed)
            };
            for line in seed {
                let _ = log.write_line(&line);
            }
            for line in batch {
                let _ = log.write_line(&line);
            }
            if missed > 0 {
                let _ = log.write_line(&format!("[dropped:{missed}]\n"));
            }
            let _ = log.flush();
            let mut q = shared.state.lock().unwrap_or_else(PoisonError::into_inner);
            q.flushed_epoch = q.flushed_epoch.max(epoch);
            shared.wake.notify_all();
            if closing && q.seed.is_empty() && q.lines.is_empty() && q.dropped == 0 {
                break;
            }
        }
        let _ = log.flush();
    }
}

impl FileSink for RotatingWriter {
    fn seed_lines(&self, lines: Vec<String>) {
        if lines.is_empty() {
            return;
        }
        let mut q = self
            .shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        q.submitted_epoch = q.submitted_epoch.saturating_add(1);
        q.seed.extend(lines);
        self.shared.wake.notify_one();
    }

    fn write_line(&self, line: &str) {
        let in_panic = IN_PANIC.try_with(Cell::get).unwrap_or(true);
        let mut q = if in_panic {
            match self.shared.state.try_lock() {
                Ok(guard) => guard,
                Err(TryLockError::Poisoned(err)) => err.into_inner(),
                Err(TryLockError::WouldBlock) => return,
            }
        } else {
            self.shared
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
        };
        if q.closed {
            return;
        }
        q.submitted_epoch = q.submitted_epoch.saturating_add(1);
        if q.lines.len() < self.shared.capacity {
            q.lines.push_back(line.to_owned());
        } else {
            q.dropped = q.dropped.saturating_add(1);
        }
        self.shared.wake.notify_one();
    }

    fn flush(&self) {
        if std::thread::current().id() == self.worker_thread {
            return;
        }
        let mut q = match self.shared.state.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(err)) => err.into_inner(),
            Err(TryLockError::WouldBlock) => {
                if IN_PANIC.try_with(Cell::get).unwrap_or(true) {
                    return;
                }
                self.shared
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
            }
        };
        if q.closed {
            return;
        }
        q.submitted_epoch = q.submitted_epoch.saturating_add(1);
        let target = q.submitted_epoch;
        q.flush_requested = true;
        self.shared.wake.notify_one();
        let _ = self
            .shared
            .wake
            .wait_timeout_while(q, Duration::from_millis(500), |state| {
                state.flushed_epoch < target && !state.closed
            });
    }

    fn close(&self) {
        self.signal_close();
        if std::thread::current().id() == self.worker_thread {
            return;
        }
        if let Some(handle) = self
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            let _ = handle.join();
        }
    }
}

pub(crate) async fn deliver_export(
    create_picker: bool,
    file_name: String,
    content: String,
    filter_label: &'static str,
) -> ExportOutcome {
    #[cfg(target_os = "android")]
    {
        let _ = (create_picker, filter_label);
        if crate::host::backup_documents::create_text_supported()
            && let Some(staged) = prepare_share_file(&file_name, &content).await
        {
            match crate::host::backup_documents::save_diagnostic_log(staged).await {
                Ok(true) => return ExportOutcome::Saved,
                Ok(false) => return ExportOutcome::Canceled,
                Err(_) => {}
            }
        }
    }
    #[cfg(not(target_os = "android"))]
    if create_picker {
        use nsh_core::file_dialog::{FileFilter, SaveDialog};
        let picked = nsh_core::element::pick_save_path(
            SaveDialog::new()
                .file_name(&file_name)
                .filter(FileFilter::new(filter_label, ["txt"])),
        )
        .await;
        match picked {
            Ok(Some(destination)) => return write_export_file(destination, content).await,
            Ok(None) => return ExportOutcome::Canceled,
            // If the desktop host has no portal/file dialog backend, fall through
            // to system share or clipboard instead of failing outright.
            Err(_) => {}
        }
    }
    #[cfg(target_os = "android")]
    let allow_share = crate::host::backup_documents::share_supported();
    #[cfg(not(target_os = "android"))]
    let allow_share = true;
    if allow_share && nsh_core::element::share_available().await {
        let temp_file = prepare_share_file(&file_name, &content).await;
        let request = match &temp_file {
            Some(path) => nsh_core::share::ShareRequest::new()
                .title(file_name.clone())
                .file(path.clone()),
            None => nsh_core::share::ShareRequest::new()
                .title(file_name.clone())
                .text(content.clone()),
        };
        let outcome = nsh_core::element::share(request).await;
        match outcome {
            Ok(outcome) => match outcome.action {
                nsh_core::share::ShareAction::Shared => {
                    // Retain `temp_file` through `SHARE_RETENTION` so the chosen
                    // Android/iOS target app can read the `FilesProvider` URI
                    // asynchronously even if the user exports a second time.
                    return ExportOutcome::Shared;
                }
                _ => {
                    if let Some(path) = temp_file {
                        let _ = fs::remove_file(path);
                    }
                    return ExportOutcome::Canceled;
                }
            },
            Err(_) => {
                if let Some(path) = temp_file {
                    let _ = fs::remove_file(path);
                }
            }
        }
    }
    nsh_core::element::set_clipboard(content);
    ExportOutcome::CopiedToClipboard
}

async fn write_export_file(destination: PathBuf, content: String) -> ExportOutcome {
    let (tx, rx) = futures_channel::oneshot::channel();
    let spawned = super::background("iptv-diag-export", move || {
        let res = write_file_atomic_or_direct(&destination, content.as_bytes());
        let _ = tx.send(res.is_ok());
    });
    if spawned.is_err() {
        return ExportOutcome::Failed;
    }
    if rx.await.unwrap_or(false) {
        ExportOutcome::Saved
    } else {
        ExportOutcome::Failed
    }
}

/// Writes `bytes` to `destination` via an atomic sibling temporary file so any
/// pre-existing file at `destination` stays intact if writing fails; only falls
/// back to direct creation when `destination` does not exist yet (`create_new`).
pub(crate) fn write_file_atomic_or_direct(destination: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Ok(meta) = fs::symlink_metadata(destination)
        && is_reparse_or_symlink(&meta)
    {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    let mut stage_err = None;
    if let Some(parent) = destination.parent() {
        let pid = std::process::id();
        for _ in 0..8 {
            let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
            let tmp = parent.join(format!(".patina-diagnostics.tmp.{pid}.{seq}"));
            match create_new_temp_file(&tmp) {
                Ok(mut file) => {
                    let res = (|| -> io::Result<()> {
                        file.write_all(bytes)?;
                        file.flush()?;
                        drop(file);
                        fs::rename(&tmp, destination)
                    })();
                    if res.is_ok() {
                        return Ok(());
                    }
                    let _ = fs::remove_file(&tmp);
                    stage_err = res.err();
                    break;
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => {
                    stage_err = Some(err);
                    break;
                }
            }
        }
    }
    match create_new_temp_file(destination) {
        Ok(mut file) => {
            let res = (|| -> io::Result<()> {
                file.write_all(bytes)?;
                file.flush()
            })();
            if res.is_err() {
                let _ = fs::remove_file(destination);
            }
            res
        }
        Err(err) => Err(stage_err.unwrap_or(err)),
    }
}

pub(crate) fn stage_share_file_in(
    data_dir: &Path,
    file_name: &str,
    bytes: &[u8],
) -> io::Result<PathBuf> {
    let stem: String = file_name
        .strip_suffix(".txt")
        .unwrap_or(file_name)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    if stem.is_empty() {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    let dir = ensure_diagnostics_dir(data_dir)?;
    prune_expired_share_files(&dir);
    let pid = std::process::id();
    let mut last_err = None;
    for _ in 0..16 {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("{stem}-{pid}-{seq}.txt"));
        match create_new_temp_file(&path) {
            Ok(mut file) => {
                let res = (|| -> io::Result<()> {
                    file.write_all(bytes)?;
                    file.flush()
                })();
                if let Err(err) = res {
                    let _ = fs::remove_file(&path);
                    return Err(err);
                }
                return Ok(path);
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                last_err = Some(err);
            }
            Err(err) => return Err(err),
        }
    }
    Err(last_err.unwrap_or_else(|| io::Error::from(io::ErrorKind::AlreadyExists)))
}

async fn prepare_share_file(file_name: &str, content: &str) -> Option<PathBuf> {
    let data_dir = try_panic_dir()?;
    let name = file_name.to_owned();
    let bytes = content.as_bytes().to_vec();
    let (tx, rx) = futures_channel::oneshot::channel();
    let spawned = super::background("iptv-diag-share", move || {
        let _ = tx.send(stage_share_file_in(&data_dir, &name, &bytes).ok());
    });
    if spawned.is_err() {
        return None;
    }
    rx.await.ok().flatten()
}

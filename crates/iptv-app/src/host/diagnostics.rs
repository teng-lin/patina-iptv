//! Composable diagnostic logger (`Option B` in #425), bounded redacted ring,
//! panic persistence hooks, desktop log rotation and user-initiated export.
//!
//! Why Option B: installing Patina's [`DiagnosticLogger`] before NSH's runner
//! starts keeps all diagnostic buffering, redaction, panic persistence and
//! desktop rotation inside `patina-iptv` without requiring a cross-repo
//! `nsh.rev` bump, while forwarding every record to the platform's own sink
//! (`__android_log_write` on Android, `os_log` + `env_logger`/stderr on Apple,
//! `env_logger` on Linux and Windows debug builds, and `js_log` on browser web).

pub(crate) mod buffer;
pub(crate) mod redact;

use std::path::Path;
#[cfg(any(not(target_family = "wasm"), test))]
use std::sync::Arc;
#[cfg(not(target_family = "wasm"))]
use std::sync::TryLockError;
use std::sync::{Mutex, PoisonError, RwLock};
use std::time::Duration;

pub(crate) use buffer::{DiagnosticHeader, DiagnosticLog};

/// Non-blocking sink for mirrored formatted lines (desktop `patina.log`).
#[cfg(any(not(target_family = "wasm"), test))]
pub(crate) trait FileSink: Send + Sync {
    fn seed_lines(&self, lines: Vec<String>);
    fn write_line(&self, line: &str);
    fn flush(&self);
    fn close(&self);
}

#[derive(Clone)]
enum InnerSink {
    #[cfg(target_family = "wasm")]
    Web,
    #[cfg(not(target_family = "wasm"))]
    Platform,
    #[cfg(test)]
    Shared(Arc<dyn log::Log>),
}

impl InnerSink {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        match self {
            #[cfg(target_family = "wasm")]
            Self::Web => wasm_support::platform_enabled(_metadata),
            #[cfg(not(target_family = "wasm"))]
            Self::Platform => super::native::diagnostics::platform_enabled(_metadata),
            #[cfg(test)]
            Self::Shared(inner) => inner.enabled(_metadata),
        }
    }

    fn log(&self, _record: &log::Record<'_>) {
        match self {
            #[cfg(target_family = "wasm")]
            Self::Web => wasm_support::platform_log(_record),
            #[cfg(not(target_family = "wasm"))]
            Self::Platform => super::native::diagnostics::platform_log(_record),
            #[cfg(test)]
            Self::Shared(inner) => {
                if inner.enabled(_record.metadata()) {
                    inner.log(_record);
                }
            }
        }
    }

    fn flush(&self) {
        match self {
            #[cfg(target_family = "wasm")]
            Self::Web => {}
            #[cfg(not(target_family = "wasm"))]
            Self::Platform => super::native::diagnostics::platform_flush(),
            #[cfg(test)]
            Self::Shared(inner) => inner.flush(),
        }
    }
}

/// Forwarding `log::Log` implementation that captures redacted `Info`+ events
/// into [`DiagnosticLog`] while passing all records on to an inner platform sink.
pub(crate) struct DiagnosticLogger {
    buffer: Mutex<DiagnosticLog>,
    roots: RwLock<Vec<String>>,
    inner: RwLock<InnerSink>,
    #[cfg(any(not(target_family = "wasm"), test))]
    file_sink: RwLock<Option<Arc<dyn FileSink>>>,
    clock: fn() -> Duration,
}

impl DiagnosticLogger {
    const fn new_default() -> Self {
        #[cfg(not(target_family = "wasm"))]
        let sink = InnerSink::Platform;
        #[cfg(target_family = "wasm")]
        let sink = InnerSink::Web;
        Self::with_default_sink(sink)
    }

    const fn with_default_sink(sink: InnerSink) -> Self {
        Self {
            buffer: Mutex::new(DiagnosticLog::new()),
            roots: RwLock::new(Vec::new()),
            inner: RwLock::new(sink),
            #[cfg(any(not(target_family = "wasm"), test))]
            file_sink: RwLock::new(None),
            clock: elapsed,
        }
    }

    /// Constructs an isolated logger forwarding to `inner` with an explicit clock.
    #[cfg(test)]
    pub(crate) fn with_inner(inner: Arc<dyn log::Log>, clock: fn() -> Duration) -> Self {
        Self {
            buffer: Mutex::new(DiagnosticLog::new()),
            roots: RwLock::new(Vec::new()),
            inner: RwLock::new(InnerSink::Shared(inner)),
            file_sink: RwLock::new(None),
            clock,
        }
    }

    pub(crate) fn set_roots(&self, raw_roots: impl IntoIterator<Item = String>) {
        let normalized = redact::normalize_roots(raw_roots);
        *self.roots.write().unwrap_or_else(PoisonError::into_inner) = normalized;
    }

    pub(crate) fn roots_snapshot(&self) -> Vec<String> {
        self.roots
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Non-blocking read of redaction roots for panic-hook safety: never waits
    /// on a write lock held by the panicking thread.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn try_roots_snapshot(&self) -> Vec<String> {
        match self.roots.try_read() {
            Ok(guard) => guard.clone(),
            Err(TryLockError::Poisoned(err)) => err.into_inner().clone(),
            Err(TryLockError::WouldBlock) => Vec::new(),
        }
    }

    /// Attaches a background file sink atomically under `buffer` -> `file_sink`
    /// lock order so no concurrent log record falls between snapshot and attach,
    /// and seeds existing startup lines uncapped.
    #[cfg(any(not(target_family = "wasm"), test))]
    pub(crate) fn attach_file_sink(&self, sink: Arc<dyn FileSink>) {
        let buf = self.buffer.lock().unwrap_or_else(PoisonError::into_inner);
        let existing = buf.formatted_lines();
        sink.seed_lines(existing);
        *self
            .file_sink
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(sink);
    }

    /// Detaches and closes the active file sink so worker threads and file
    /// handles terminate cleanly on shutdown.
    #[cfg(any(not(target_family = "wasm"), test))]
    pub(crate) fn close_file_sink(&self) {
        let sink = self
            .file_sink
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(sink) = sink {
            sink.close();
        }
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .flush();
    }

    /// Non-blocking panic record append: skips the in-memory lock if the current
    /// thread panicked while holding it, avoiding self-deadlock.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn record_panic_nonblocking(&self, summary: &str, roots: &[String]) {
        let now = (self.clock)();
        let formatted = match self.buffer.try_lock() {
            Ok(mut buf) => buf.push(now, log::Level::Error, "panic", summary, roots),
            Err(TryLockError::Poisoned(err)) => {
                err.into_inner()
                    .push(now, log::Level::Error, "panic", summary, roots)
            }
            Err(TryLockError::WouldBlock) => {
                let clean = redact::redact_text(summary, roots);
                let level = log::Level::Error;
                let target = "panic";
                Some(format!(
                    "+{}.{:03}s {level:<5} {target}: {clean}\n",
                    now.as_secs(),
                    now.subsec_millis()
                ))
            }
        };
        if let Some(line) = formatted
            && let Ok(guard) = self.file_sink.try_read()
            && let Some(sink) = guard.as_ref()
        {
            sink.write_line(&line);
            sink.flush();
        }
    }

    pub(crate) fn export(&self, header: &DiagnosticHeader, last_panic: Option<&str>) -> String {
        let roots = self.roots_snapshot();
        self.buffer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .format_export(header, last_panic, &roots)
    }
}

impl log::Log for DiagnosticLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        if DiagnosticLog::accepts(metadata.level()) {
            return true;
        }
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .enabled(metadata)
    }

    fn log(&self, record: &log::Record<'_>) {
        let accepts_ring = DiagnosticLog::accepts(record.level());
        let inner = self
            .inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let forward_inner = inner.enabled(record.metadata());
        if !accepts_ring && !forward_inner {
            return;
        }
        let raw_message = record.args().to_string();
        let roots = self.roots_snapshot();
        if forward_inner {
            let clean_target = redact::redact_text(record.target(), &roots);
            let clean_msg = redact::redact_text(&raw_message, &roots);
            inner.log(
                &log::Record::builder()
                    .level(record.level())
                    .target(&clean_target)
                    .args(format_args!("{clean_msg}"))
                    .line(record.line())
                    .build(),
            );
        }
        if !accepts_ring {
            return;
        }
        let now = (self.clock)();
        let buf_guard = self.buffer.lock();
        let mut buf = buf_guard.unwrap_or_else(PoisonError::into_inner);
        let formatted = buf.push(now, record.level(), record.target(), &raw_message, &roots);
        #[cfg(any(not(target_family = "wasm"), test))]
        if let Some(line) = formatted
            && let Some(sink) = self
                .file_sink
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        {
            sink.write_line(&line);
        }
        #[cfg(all(target_family = "wasm", not(test)))]
        let _ = formatted;
    }

    fn flush(&self) {
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .flush();
        #[cfg(any(not(target_family = "wasm"), test))]
        if let Some(sink) = self
            .file_sink
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            sink.flush();
        }
    }
}

static LOGGER: DiagnosticLogger = DiagnosticLogger::new_default();

/// Early lock-free registration used by Android's `.init_array` before `JNI_OnLoad`.
#[cfg(target_os = "android")]
pub(crate) fn install_logger_early() {
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Info);
}

/// Outcome of delivering a user-requested diagnostic export.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(target_family = "wasm", allow(dead_code))]
pub(crate) enum ExportOutcome {
    Saved,
    Shared,
    CopiedToClipboard,
    Canceled,
    Failed,
}

/// Display name for `platform` in diagnostic export headers.
pub(crate) const fn platform_label(platform: iptv_core::capabilities::Platform) -> &'static str {
    use iptv_core::capabilities::Platform;
    match platform {
        Platform::Android => "android",
        Platform::Linux => "linux",
        Platform::Macos => "macos",
        Platform::Windows => "windows",
        Platform::Ipad => "ipad",
        Platform::Unknown => {
            if cfg!(target_family = "wasm") {
                "web"
            } else {
                "unknown"
            }
        }
    }
}

/// Whether `general.send_logs` has a working user-facing delivery transport on
/// this host (save picker, non-stub share sheet, or system clipboard).
pub(crate) fn export_offered(host: iptv_core::capabilities::Host, create_picker: bool) -> bool {
    if super::is_tui_mode() {
        return false;
    }
    if cfg!(all(target_family = "wasm", feature = "browser-package")) {
        return false;
    }
    #[cfg(target_os = "android")]
    {
        let _ = create_picker;
        if host.form_factor == iptv_core::capabilities::FormFactor::Tv
            && !super::backup_documents::create_text_supported()
            && !super::backup_documents::share_supported()
        {
            return false;
        }
    }
    #[cfg(not(target_os = "android"))]
    let _ = (host, create_picker);
    true
}

/// Installs the global forwarding logger and panic hook (idempotent).
pub fn install() {
    #[cfg(not(target_family = "wasm"))]
    super::native::diagnostics::install(&LOGGER);
    #[cfg(target_family = "wasm")]
    wasm_support::install(&LOGGER);
}

/// Monotonic time since diagnostics started in this process.
pub(crate) fn elapsed() -> Duration {
    #[cfg(not(target_family = "wasm"))]
    {
        super::native::diagnostics::elapsed()
    }
    #[cfg(target_family = "wasm")]
    {
        wasm_support::elapsed()
    }
}

/// Seeds redaction roots for `directory` without arming file writes yet.
pub(crate) fn seed_roots(directory: &Path) {
    #[cfg(not(target_family = "wasm"))]
    super::native::diagnostics::seed_roots(&LOGGER, directory);
    #[cfg(target_family = "wasm")]
    wasm_support::set_panic_dir(&LOGGER, directory);
}

/// Records the private storage path for panic persistence without opening files.
pub(crate) fn set_panic_dir(directory: &Path) {
    #[cfg(not(target_family = "wasm"))]
    super::native::diagnostics::set_panic_dir(&LOGGER, directory);
    #[cfg(target_family = "wasm")]
    wasm_support::set_panic_dir(&LOGGER, directory);
}

/// Activates post-recovery diagnostics: checks `last-panic.txt` and starts
/// desktop log rotation once per locked storage root.
pub(crate) fn activate_storage(directory: &Path) {
    #[cfg(not(target_family = "wasm"))]
    super::native::diagnostics::activate_storage(&LOGGER, directory);
    #[cfg(target_family = "wasm")]
    wasm_support::activate_storage(&LOGGER, directory);
}

/// Flushes buffered diagnostic file writes before backgrounding or exit.
pub fn flush() {
    log::Log::flush(&LOGGER);
}

/// Flushes, detaches and closes background file writers during app teardown.
pub(crate) fn shutdown() {
    #[cfg(not(target_family = "wasm"))]
    super::native::diagnostics::shutdown(&LOGGER);
    #[cfg(target_family = "wasm")]
    flush();
}

/// Formats the current diagnostic snapshot (header + `last-panic.txt` + buffer).
pub(crate) fn export(header: &DiagnosticHeader) -> String {
    #[cfg(not(target_family = "wasm"))]
    let last_panic = super::native::diagnostics::read_last_panic();
    #[cfg(target_family = "wasm")]
    let last_panic: Option<String> = None;
    LOGGER.export(header, last_panic.as_deref())
}

/// Delivers `content` via save picker, system share sheet, or clipboard fallback.
pub(crate) async fn deliver_export(
    create_picker: bool,
    file_name: String,
    content: String,
    filter_label: &'static str,
) -> ExportOutcome {
    #[cfg(not(target_family = "wasm"))]
    {
        super::native::diagnostics::deliver_export(create_picker, file_name, content, filter_label)
            .await
    }
    #[cfg(target_family = "wasm")]
    {
        let _ = (create_picker, filter_label);
        wasm_support::deliver_export(file_name, content).await
    }
}

#[cfg(target_family = "wasm")]
mod wasm_support {
    use std::path::Path;
    use std::time::Duration;

    use super::{DiagnosticLogger, ExportOutcome};

    pub(super) fn platform_enabled(metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    pub(super) fn platform_log(record: &log::Record<'_>) {
        if record.level() > log::Level::Info {
            return;
        }
        #[cfg(not(feature = "browser-package"))]
        {
            #[link(wasm_import_module = "nsh")]
            unsafe extern "C" {
                #[link_name = "log"]
                fn js_log(level: u32, message: *const u8, len: usize);
            }
            let message = record.args().to_string();
            // SAFETY: `message` bytes remain valid across the synchronous JS import call.
            unsafe {
                js_log(record.level() as u32, message.as_ptr(), message.len());
            }
        }
    }

    pub(super) fn install(logger: &'static DiagnosticLogger) {
        // On Tizen (`browser-package`), leave `nsh-platform-tizen`'s worker logger
        // in place since diagnostic export is hidden there.
        #[cfg(feature = "browser-package")]
        {
            let _ = logger;
        }
        #[cfg(not(feature = "browser-package"))]
        {
            logger.set_roots([super::super::web::DIAGNOSTIC_DIR.to_owned()]);
            if log::set_logger(logger).is_ok() {
                log::set_max_level(log::LevelFilter::Info);
            }
        }
    }

    pub(super) fn elapsed() -> Duration {
        iptv_web::adapters::monotonic()
    }

    pub(super) fn set_panic_dir(logger: &DiagnosticLogger, directory: &Path) {
        logger.set_roots([directory.to_string_lossy().into_owned()]);
    }

    pub(super) fn activate_storage(logger: &DiagnosticLogger, directory: &Path) {
        set_panic_dir(logger, directory);
    }

    pub(super) async fn deliver_export(file_name: String, content: String) -> ExportOutcome {
        if nsh_core::element::share_available().await {
            let request = nsh_core::share::ShareRequest::new()
                .title(file_name)
                .text(content.clone());
            if let Ok(outcome) = nsh_core::element::share(request).await {
                return match outcome.action {
                    nsh_core::share::ShareAction::Shared => ExportOutcome::Shared,
                    _ => ExportOutcome::Canceled,
                };
            }
        }
        nsh_core::element::set_clipboard(content);
        ExportOutcome::CopiedToClipboard
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests;

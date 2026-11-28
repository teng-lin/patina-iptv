//! Bounded, redacted in-memory diagnostic log buffer and export formatter.
//!
//! Portable across native and `wasm32-unknown-unknown` builds: uses no OS
//! clocks or filesystem APIs directly.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::time::Duration;

#[cfg(any(not(target_family = "wasm"), test))]
use super::redact::crate_relative_location;
use super::redact::redact_text;

/// Maximum number of diagnostic records kept in memory.
pub(crate) const MAX_RECORDS: usize = 2_000;
/// Maximum UTF-8 byte budget across retained diagnostic records (512 KiB).
pub(crate) const MAX_BYTES: usize = 512 * 1024;
/// Ceiling for a single record's message so one giant entry cannot flush the ring.
const MAX_RECORD_MESSAGE_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiagnosticRecord {
    pub elapsed: Duration,
    pub level: log::Level,
    pub target: String,
    pub message: String,
    bytes: usize,
}

impl DiagnosticRecord {
    pub(crate) fn format_into(&self, out: &mut String) {
        let secs = self.elapsed.as_secs();
        let millis = self.elapsed.subsec_millis();
        let _ = writeln!(
            out,
            "+{secs}.{millis:03}s {:<5} {}: {}",
            self.level, self.target, self.message
        );
    }
}

/// Safe metadata header written at the top of every diagnostic export.
/// Contains strictly build, platform, form-factor, uptime and numeric queue facts.
#[derive(Clone, Debug)]
pub(crate) struct DiagnosticHeader {
    pub app_version: &'static str,
    pub git_hash: &'static str,
    pub store_schema_version: u32,
    pub settings_schema_version: u32,
    pub nsh_rev: &'static str,
    pub platform: &'static str,
    pub target: &'static str,
    pub form_factor: &'static str,
    pub window_class: &'static str,
    pub uptime: Duration,
    pub store_lanes: Option<[iptv_runtime::model::StoreQueueStats; 3]>,
}

impl DiagnosticHeader {
    pub(crate) fn format_into(&self, out: &mut String) {
        let _ = writeln!(out, "[patina-diagnostics]");
        let _ = writeln!(out, "app=v{}({})", self.app_version, self.git_hash);
        let _ = writeln!(
            out,
            "schema=store:{},settings:{}",
            self.store_schema_version, self.settings_schema_version
        );
        let _ = writeln!(out, "nsh_rev={}", self.nsh_rev);
        let _ = writeln!(out, "platform={}", self.platform);
        let _ = writeln!(out, "target={}", self.target);
        let _ = writeln!(out, "form_factor={}", self.form_factor);
        let _ = writeln!(out, "window={}", self.window_class);
        let _ = writeln!(
            out,
            "uptime={}.{:03}s",
            self.uptime.as_secs(),
            self.uptime.subsec_millis()
        );
        if let Some(lanes) = &self.store_lanes {
            let _ = writeln!(out, "[store-lanes]");
            for lane in lanes {
                let _ = writeln!(
                    out,
                    "lane.{}=pending:{},oldest_ms:{},max_wait_ms:{},skipped:{}",
                    lane.lane,
                    lane.pending,
                    lane.oldest_age.as_millis(),
                    lane.max_wait.as_millis(),
                    lane.skipped_reads,
                );
            }
        }
    }
}

/// Bounded FIFO buffer of `Info`, `Warn` and `Error` records.
pub(crate) struct DiagnosticLog {
    records: VecDeque<DiagnosticRecord>,
    total_bytes: usize,
    evicted_records: u64,
    max_records: usize,
    max_bytes: usize,
}

impl Default for DiagnosticLog {
    fn default() -> Self {
        Self::new()
    }
}

impl DiagnosticLog {
    pub(crate) const fn new() -> Self {
        Self::with_limits(MAX_RECORDS, MAX_BYTES)
    }

    pub(crate) const fn with_limits(max_records: usize, max_bytes: usize) -> Self {
        Self {
            records: VecDeque::new(),
            total_bytes: 0,
            evicted_records: 0,
            max_records,
            max_bytes,
        }
    }

    /// Whether `level` is stored in diagnostics (`Info`, `Warn` and `Error`).
    pub(crate) fn accepts(level: log::Level) -> bool {
        level <= log::Level::Info
    }

    /// Redacts and stores one record, evicting the oldest entries first when
    /// either the record-count or byte ceiling would be exceeded.
    /// Returns the formatted single-line entry when stored.
    pub(crate) fn push(
        &mut self,
        elapsed: Duration,
        level: log::Level,
        target: &str,
        raw_message: &str,
        roots: &[String],
    ) -> Option<String> {
        if !Self::accepts(level) || self.max_records == 0 || self.max_bytes == 0 {
            return None;
        }
        let clean_target = single_line(&redact_text(target, roots), 256);
        let secs = elapsed.as_secs();
        let millis = elapsed.subsec_millis();
        let mut prefix = String::with_capacity(32 + clean_target.len());
        let _ = write!(prefix, "+{secs}.{millis:03}s {level:<5} {clean_target}: ");
        let framing_bytes = prefix.len() + 1;
        if framing_bytes > self.max_bytes {
            return None;
        }
        let max_msg = MAX_RECORD_MESSAGE_BYTES.min(self.max_bytes - framing_bytes);
        let clean_message = single_line(&redact_text(raw_message, roots), max_msg);
        let bytes = framing_bytes + clean_message.len();
        while !self.records.is_empty()
            && (self.records.len() >= self.max_records || self.total_bytes + bytes > self.max_bytes)
        {
            if let Some(removed) = self.records.pop_front() {
                self.total_bytes = self.total_bytes.saturating_sub(removed.bytes);
                self.evicted_records += 1;
            }
        }
        let mut line = prefix;
        line.push_str(&clean_message);
        line.push('\n');
        debug_assert_eq!(line.len(), bytes);
        let record = DiagnosticRecord {
            elapsed,
            level,
            target: clean_target,
            message: clean_message,
            bytes,
        };
        self.total_bytes += record.bytes;
        self.records.push_back(record);
        Some(line)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }

    #[cfg(test)]
    pub(crate) fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    #[cfg(test)]
    pub(crate) fn evicted_records(&self) -> u64 {
        self.evicted_records
    }

    /// Snapshot all currently buffered lines in chronological order (oldest first).
    #[cfg(any(not(target_family = "wasm"), test))]
    pub(crate) fn formatted_lines(&self) -> Vec<String> {
        let mut lines = Vec::with_capacity(self.records.len());
        for record in &self.records {
            let mut line = String::with_capacity(record.bytes);
            record.format_into(&mut line);
            lines.push(line);
        }
        lines
    }

    /// Formats a complete diagnostic export: header, optional previous panic,
    /// and buffered records from oldest to newest.
    pub(crate) fn format_export(
        &self,
        header: &DiagnosticHeader,
        last_panic: Option<&str>,
        roots: &[String],
    ) -> String {
        let mut out = String::with_capacity(self.total_bytes + 1024);
        header.format_into(&mut out);
        if let Some(panic_text) = last_panic.map(str::trim).filter(|s| !s.is_empty()) {
            let _ = writeln!(out);
            let _ = writeln!(out, "[last-panic]");
            for line in panic_text.lines() {
                let _ = writeln!(out, "{}", redact_text(line, roots));
            }
        }
        let _ = writeln!(out);
        let _ = writeln!(out, "[events]");
        if self.evicted_records > 0 {
            let _ = writeln!(out, "evicted_records={}", self.evicted_records);
        }
        for record in &self.records {
            record.format_into(&mut out);
        }
        out
    }
}

fn single_line(text: &str, max_bytes: usize) -> String {
    let collapsed: String = text
        .chars()
        .map(|c| match c {
            '\n' | '\r' | '\t' => ' ',
            other if other.is_control() => ' ',
            other => other,
        })
        .collect();
    let trimmed = collapsed.trim();
    if trimmed.len() <= max_bytes {
        return trimmed.to_owned();
    }
    const SUFFIX: &str = "...[truncated]";
    if max_bytes <= SUFFIX.len() {
        return truncate_utf8(trimmed, max_bytes).to_owned();
    }
    let head = truncate_utf8(trimmed, max_bytes - SUFFIX.len());
    format!("{head}{SUFFIX}")
}

fn truncate_utf8(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Formats a redacted panic report suitable for `last-panic.txt`.
#[cfg(any(not(target_family = "wasm"), test))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn format_panic_report(
    app_version: &str,
    git_hash: &str,
    nsh_rev: &str,
    platform: &str,
    target: &str,
    uptime: Duration,
    thread_name: &str,
    location: Option<(&str, u32, u32)>,
    raw_message: &str,
    backtrace: Option<&str>,
    roots: &[String],
) -> (String, String) {
    let thread = single_line(&redact_text(thread_name, roots), 128);
    let loc_str = match location {
        Some((file, line, col)) => {
            let rel = crate_relative_location(file);
            format!("{}:{line}:{col}", redact_text(&rel, roots))
        }
        None => "unknown".to_owned(),
    };
    let message = single_line(&redact_text(raw_message, roots), 4096);
    let mut summary = String::new();
    let _ = write!(summary, "thread={thread}");
    summary.push(' ');
    let _ = write!(summary, "location={loc_str}");
    summary.push(' ');
    let _ = write!(summary, "message={message}");
    let mut report = String::with_capacity(512);
    let _ = writeln!(report, "app=v{app_version}({git_hash})");
    let _ = writeln!(report, "nsh_rev={nsh_rev}");
    let _ = writeln!(report, "platform={platform}");
    let _ = writeln!(report, "target={target}");
    let _ = writeln!(
        report,
        "uptime={}.{:03}s",
        uptime.as_secs(),
        uptime.subsec_millis()
    );
    let _ = writeln!(report, "thread={thread}");
    let _ = writeln!(report, "location={loc_str}");
    let _ = writeln!(report, "message={message}");
    if let Some(bt) = backtrace.filter(|s| !s.trim().is_empty()) {
        let _ = writeln!(report, "[backtrace]");
        report.push_str(&super::redact::strip_backtrace_paths(bt, roots));
    }
    (summary, report)
}

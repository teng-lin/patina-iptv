use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::buffer::{DiagnosticHeader, DiagnosticLog, MAX_BYTES, MAX_RECORDS};
use super::redact::{crate_relative_location, normalize_roots, redact_text, strip_backtrace_paths};
use super::{DiagnosticLogger, FileSink};
use crate::host::native::diagnostics::{
    ROTATE_BYTES, RotatingLog, RotatingWriter, os_log_parts, read_last_panic_in,
    stage_share_file_in, summarize_previous_panic, write_file_atomic_or_direct,
    write_last_panic_atomic,
};

fn sample_header() -> DiagnosticHeader {
    DiagnosticHeader {
        app_version: "0.9.0",
        git_hash: "0123abcd",
        store_schema_version: 14,
        settings_schema_version: 6,
        nsh_rev: "deadbeef01234567",
        platform: "linux",
        target: "x86_64-unknown-linux-gnu",
        form_factor: "desktop",
        window_class: "expanded",
        uptime: Duration::from_millis(42_500),
        store_lanes: Some([
            iptv_runtime::model::StoreQueueStats {
                lane: "write",
                pending: 0,
                oldest_age: Duration::ZERO,
                max_wait: Duration::from_millis(4),
                skipped_reads: 0,
            },
            iptv_runtime::model::StoreQueueStats {
                lane: "read",
                pending: 1,
                oldest_age: Duration::from_millis(2),
                max_wait: Duration::from_millis(9),
                skipped_reads: 3,
            },
            iptv_runtime::model::StoreQueueStats {
                lane: "background",
                pending: 0,
                oldest_age: Duration::ZERO,
                max_wait: Duration::from_millis(12),
                skipped_reads: 0,
            },
        ]),
    }
}

#[test]
fn diagnostic_log_enforces_record_and_byte_ceilings_fifo() {
    let mut by_count = DiagnosticLog::with_limits(3, MAX_BYTES);
    for idx in 0..5 {
        by_count.push(
            Duration::from_millis(idx * 100),
            log::Level::Info,
            "iptv_test",
            &format!("synthetic-entry-{idx}"),
            &[],
        );
    }
    assert_eq!(by_count.len(), 3);
    assert_eq!(by_count.evicted_records(), 2);
    let lines = by_count.formatted_lines();
    assert!(lines[0].contains("synthetic-entry-2"), "{lines:?}");
    assert!(lines[1].contains("synthetic-entry-3"), "{lines:?}");
    assert!(lines[2].contains("synthetic-entry-4"), "{lines:?}");

    let mut by_bytes = DiagnosticLog::with_limits(MAX_RECORDS, 180);
    for idx in 0..6 {
        by_bytes.push(
            Duration::from_secs(idx),
            log::Level::Warn,
            "iptv_mem",
            &format!("payload-{idx:02}-abcdefghijklmnopqrstuvwxyz"),
            &[],
        );
        assert!(by_bytes.total_bytes() <= 180);
    }
    assert!(by_bytes.evicted_records() > 0);
    let kept = by_bytes.formatted_lines().join("");
    assert!(!kept.contains("payload-00"), "{kept}");
    assert!(kept.contains("payload-05"), "{kept}");
    assert_eq!(kept.len(), by_bytes.total_bytes());

    // Multi-byte UTF-8 truncation never splits a code point or exceeds the ceiling.
    let mut utf8_ring = DiagnosticLog::with_limits(MAX_RECORDS, 96);
    let multi = "合成診断メッセージ".repeat(20);
    let stored = utf8_ring
        .push(Duration::from_secs(1), log::Level::Error, "t", &multi, &[])
        .expect("truncated utf8 record fits");
    assert!(stored.len() <= 96, "len={} stored={stored}", stored.len());
    assert!(stored.contains("...[truncated]"), "{stored}");
}

#[test]
fn diagnostic_log_filters_debug_and_trace_levels() {
    let mut ring = DiagnosticLog::new();
    assert!(
        ring.push(
            Duration::from_millis(1),
            log::Level::Debug,
            "iptv_debug",
            "ignored-debug",
            &[]
        )
        .is_none()
    );
    assert!(
        ring.push(
            Duration::from_millis(2),
            log::Level::Trace,
            "iptv_trace",
            "ignored-trace",
            &[]
        )
        .is_none()
    );
    assert_eq!(ring.len(), 0);
    for level in [log::Level::Info, log::Level::Warn, log::Level::Error] {
        assert!(
            ring.push(Duration::from_millis(3), level, "iptv_ok", "kept", &[])
                .is_some()
        );
    }
    assert_eq!(ring.len(), 3);
}

#[test]
fn redaction_strips_synthetic_urls_credentials_paths_and_backtraces() {
    let roots = normalize_roots([
        "/tmp/synthetic-patina-data".to_owned(),
        "C:\\SyntheticAppData\\Patina".to_owned(),
    ]);
    let raw = concat!(
        "stream=http://syn_user:syn_pass@stream.example.test:8080/live/syn_user/syn_pass/101.ts ",
        "apos=https://apos.example.test/live/syn_auser/a'syn_asecret/42.ts ",
        "single_q='https://sq.example.test/live/syn_squ/syn_sqp/9.ts' ",
        "comma_cred=https://syn_cuser:syn_pw,tail@comma.example.test/hls/live,720p,1080p.m3u8;jsessionid=syn_jsess?username=syn_qu ",
        "paren_url=(https://paren.example.test/vod_(1)/movie.mp4) ",
        "v6=https://syn_v6:syn_v6_pass@[2001:db8::42]:8443/secret-path?token=syn_qtok ",
        "local=file:///tmp/synthetic-patina-data/playlists/generated.m3u ",
        "rtsp=rtsp://camera.example.test/ch1 ",
        "xtream_rel=/live/syn_xuser/syn_xpass/404.ts ",
        "xtream_short=/syn_short_u/syn_short_p/42.ts ",
        "xtream_host=hostxt.example.test:8080/syn_hxu/syn_hxp/77.m3u8 ",
        "auth_bare=syn_acct:syn_secret99@portal.example.test:8080/c/ ",
        "v6_bare=syn_acct:syn_secret99@[2001:db8::9]:8080 ",
        "hdr=Bearer syn-jwt-secret-token basic=Basic c3luOnBhc3M= ",
        "digest=Authorization: Digest username=\"syn_dig\", response=\"syn_resp\"\n",
        "cookie=Cookie: mac=00:1A:79:AA:BB:CC; sid=prefix}syn_brace_cookie; session_id=syn_cookie_sess\n",
        "xapi=X-Api-Key: syn-x-api-header\n",
        "kv=api_key=syn-api-key-77&password=\"syn-quoted-pass\"&mac=00:1A:79:11:22:33&device_id=syn-dev-id ",
        "json={\"password\": \"syn-json-pass\", \"pin\": \"9876\"} ",
        "dbg=Config { password: \"syn-dbg-pass\", pin: 4321, mac: \"00:1A:79:FF:EE:DD\" } ",
        "unix_data=/tmp/synthetic-patina-data/iptv.sqlite: locked ",
        "win_data=c:\\syntheticappdata\\patina\\diagnostics\\patina.log ",
        "home=/home/synthetic_alice/.config/patina/list.m3u ",
        "macos_path=/Users/Synthetic Bob/Library/Application Support/iptv-rs/db opened ",
        "win_home=C:\\Users\\Synthetic Carol\\AppData\\Local\\Patina\\db ",
        "android=/data/user/0/com.hantekllc.iptvrs/files/iptv.sqlite"
    );
    let cleaned = redact_text(raw, &roots);

    for forbidden in [
        "http://",
        "https://",
        "file://",
        "rtsp://",
        "stream.example.test",
        "apos.example.test",
        "sq.example.test",
        "comma.example.test",
        "paren.example.test",
        "camera.example.test",
        "hostxt.example.test",
        "portal.example.test",
        "2001:db8::42",
        "2001:db8::9",
        "syn_user",
        "syn_pass",
        "syn_auser",
        "syn_asecret",
        "syn_squ",
        "syn_sqp",
        "syn_cuser",
        "syn_pw,tail",
        "syn_jsess",
        "syn_qu",
        "syn_v6",
        "syn_qtok",
        "syn_xuser",
        "syn_xpass",
        "syn_short_u",
        "syn_short_p",
        "syn_hxu",
        "syn_hxp",
        "syn_acct",
        "syn_secret99",
        "syn-jwt-secret-token",
        "c3luOnBhc3M=",
        "syn_dig",
        "syn_resp",
        "00:1A:79",
        "syn_brace_cookie",
        "syn_cookie_sess",
        "syn-x-api-header",
        "syn-api-key-77",
        "syn-quoted-pass",
        "syn-dev-id",
        "syn-json-pass",
        "9876",
        "syn-dbg-pass",
        "4321",
        "/tmp/synthetic-patina-data",
        "syntheticappdata",
        "synthetic_alice",
        "Synthetic Bob",
        "Application Support",
        "Synthetic Carol",
        "com.hantekllc.iptvrs/files",
    ] {
        assert!(
            !cleaned.contains(forbidden),
            "leaked {forbidden:?} in: {cleaned}"
        );
    }
    assert!(cleaned.contains("single_q='[redacted-url]'"), "{cleaned}");
    assert!(cleaned.contains(": locked"), "{cleaned}");
    assert!(cleaned.contains("([redacted-url])"), "{cleaned}");
    assert!(cleaned.contains("[redacted-path] opened"), "{cleaned}");

    assert_eq!(
        crate_relative_location("/home/synthetic_builder/work/patina/crates/iptv-app/src/lib.rs"),
        "crates/iptv-app/src/lib.rs"
    );
    assert_eq!(
        crate_relative_location(r"C:\Users\synthetic_builder\patina\vendor\nsh-tv\src\lib.rs"),
        "vendor/nsh-tv/src/lib.rs"
    );
    let sample_bt = "\
   0: iptv_app::boom
             at /home/synthetic_builder/patina/crates/iptv-app/src/main.rs:42:9
   1: core::ops::function::FnOnce::call_once
             at /rustc/synthetic_hash/library/core/src/ops/function.rs:250:5
";
    let stripped_bt = strip_backtrace_paths(sample_bt, &roots);
    assert!(!stripped_bt.contains("synthetic_builder"), "{stripped_bt}");
    assert!(
        stripped_bt.contains("at crates/iptv-app/src/main.rs:42:9"),
        "{stripped_bt}"
    );
}

#[test]
fn export_header_and_sections_match_strict_field_allowlist() {
    let roots = normalize_roots(["/tmp/synthetic-data-root".to_owned()]);
    let mut ring = DiagnosticLog::new();
    ring.push(
        Duration::from_millis(125),
        log::Level::Info,
        "iptv_app::startup",
        "opened catalog from /tmp/synthetic-data-root/iptv.sqlite",
        &roots,
    );
    ring.push(
        Duration::from_millis(950),
        log::Level::Warn,
        "iptv_net",
        "refresh failed for https://syn_u:syn_p@provider.example.test/get.php?password=syn_p",
        &roots,
    );
    let fake_panic = "\
app=v0.9.0(0123abcd)
nsh_rev=deadbeef01234567
platform=linux
target=x86_64-unknown-linux-gnu
uptime=3.100s
thread=main
location=crates/iptv-app/src/main.rs:12:5
message=failed reading /tmp/synthetic-data-root/secret.txt from http://provider.example.test/x
";
    let exported = ring.format_export(&sample_header(), Some(fake_panic), &roots);

    const ALLOWED_PREFIXES: &[&str] = &[
        "[patina-diagnostics]",
        "app=",
        "schema=",
        "nsh_rev=",
        "platform=",
        "target=",
        "form_factor=",
        "window=",
        "uptime=",
        "[store-lanes]",
        "lane.write=",
        "lane.read=",
        "lane.background=",
        "[last-panic]",
        "thread=",
        "location=",
        "message=",
        "[events]",
        "evicted_records=",
        "+",
    ];
    for line in exported.lines().filter(|l| !l.is_empty()) {
        assert!(
            ALLOWED_PREFIXES.iter().any(|p| line.starts_with(p)),
            "unexpected diagnostic export line: {line}"
        );
    }
    for forbidden in [
        "provider.example.test",
        "syn_u",
        "syn_p",
        "/tmp/synthetic-data-root",
        "http://",
        "https://",
    ] {
        assert!(!exported.contains(forbidden), "{exported}");
    }
    let panic_pos = exported.find("[last-panic]").unwrap();
    let events_pos = exported.find("[events]").unwrap();
    let first_event = exported.find("+0.125s").unwrap();
    let second_event = exported.find("+0.950s").unwrap();
    assert!(panic_pos < events_pos && events_pos < first_event && first_event < second_event);
}

struct FakeInnerLogger {
    records: Mutex<Vec<(log::Level, String, String)>>,
    flushes: AtomicUsize,
    max_level: log::Level,
}

impl log::Log for FakeInnerLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= self.max_level
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            self.records.lock().unwrap().push((
                record.level(),
                record.target().to_owned(),
                record.args().to_string(),
            ));
        }
    }

    fn flush(&self) {
        self.flushes.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn forwarding_logger_passes_all_enabled_records_to_inner_sink() {
    let inner = Arc::new(FakeInnerLogger {
        records: Mutex::new(Vec::new()),
        flushes: AtomicUsize::new(0),
        max_level: log::Level::Debug,
    });
    let logger = DiagnosticLogger::with_inner(inner.clone(), || Duration::from_millis(500));
    logger.set_roots(["/tmp/synthetic-forward-dir".to_owned()]);

    let debug_meta = log::Metadata::builder()
        .level(log::Level::Debug)
        .target("nsh::dev")
        .build();
    let trace_meta = log::Metadata::builder()
        .level(log::Level::Trace)
        .target("nsh::dev")
        .build();
    assert!(log::Log::enabled(&logger, &debug_meta));
    assert!(!log::Log::enabled(&logger, &trace_meta));

    log::Log::log(
        &logger,
        &log::Record::builder()
            .args(format_args!("dev trace line"))
            .level(log::Level::Debug)
            .target("nsh::dev")
            .build(),
    );
    log::Log::log(
        &logger,
        &log::Record::builder()
            .args(format_args!(
                "connected https://syn_user:syn_pw@example.test/live"
            ))
            .level(log::Level::Info)
            .target("iptv_app")
            .build(),
    );
    log::Log::flush(&logger);

    let forwarded = inner.records.lock().unwrap();
    assert_eq!(forwarded.len(), 2);
    assert_eq!(forwarded[0].0, log::Level::Debug);
    assert_eq!(forwarded[0].1, "nsh::dev");
    assert_eq!(forwarded[1].0, log::Level::Info);
    assert_eq!(forwarded[1].1, "iptv_app");
    assert!(
        forwarded[1].2.contains("[redacted-url]") && !forwarded[1].2.contains("syn_pw"),
        "platform sink must receive redacted text: {:?}",
        forwarded[1]
    );
    assert_eq!(inner.flushes.load(Ordering::SeqCst), 1);

    let exported = logger.export(&sample_header(), None);
    assert!(!exported.contains("dev trace line"), "{exported}");
    assert!(exported.contains("[redacted-url]"), "{exported}");
    assert!(!exported.contains("syn_pw"), "{exported}");
}

fn log_files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("patina.log"))
        .collect();
    names.sort();
    names
}

#[test]
fn rotating_log_rolls_at_one_mib_keeping_at_most_three_files() {
    let tmp = tempfile::tempdir().unwrap();
    let mut log = RotatingLog::open(tmp.path().to_path_buf(), ROTATE_BYTES).unwrap();
    let chunk = format!("{}\n", "x".repeat(32 * 1024 - 1));
    assert_eq!(chunk.len(), 32 * 1024);

    // Write 3.5 MiB across 112 chunks of 32 KiB (> 3 rotations past 1 MiB).
    for i in 0..112 {
        let line = format!("seq={i:03} {}", &chunk[8..]);
        log.write_line(&line).unwrap();
    }
    log.flush().unwrap();

    let files = log_files_in(tmp.path());
    assert_eq!(
        files,
        vec!["patina.log", "patina.log.1", "patina.log.2"],
        "must keep at most 3 files: {files:?}"
    );
    for name in &files {
        let len = fs::metadata(tmp.path().join(name)).unwrap().len();
        assert!(
            len <= ROTATE_BYTES,
            "{name} exceeded 1 MiB ceiling ({len} > {ROTATE_BYTES})"
        );
    }
    let newest = fs::read_to_string(tmp.path().join("patina.log")).unwrap();
    assert!(
        newest.contains("seq=111"),
        "latest entry stays in patina.log"
    );
}

#[test]
fn rotating_log_preserves_active_and_history_when_active_rename_fails() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("patina.log.1"), b"preserved-gen-1\n").unwrap();
    fs::write(tmp.path().join("patina.log.2"), b"preserved-gen-2\n").unwrap();

    let mut log = RotatingLog::open(tmp.path().to_path_buf(), 128).unwrap();
    log.write_line("first-important-record-0123456789abcdef\n")
        .unwrap();
    log.write_line("second-important-record-0123456789abcdef\n")
        .unwrap();
    log.flush().unwrap();

    // Block renaming `patina.log -> patina.log.rotating` with a non-empty directory.
    let blocker = tmp.path().join("patina.log.rotating");
    fs::create_dir_all(&blocker).unwrap();
    fs::write(blocker.join("pinned"), b"hold").unwrap();

    log.write_line("third-important-record-exceeding-128-bytes-0123456789abcdef\n")
        .unwrap();
    for idx in 0..20 {
        log.write_line(&format!("overflow-line-{idx:02}-0123456789abcdef\n"))
            .unwrap();
    }
    log.flush().unwrap();

    let active = fs::read_to_string(tmp.path().join("patina.log")).unwrap();
    assert!(
        active.contains("first-important-record") && active.contains("third-important-record"),
        "failed rename must append rather than truncating patina.log: {active}"
    );
    assert!(
        active.len() <= 256,
        "fallback growth must stay bounded at 2 * max_bytes (got {})",
        active.len()
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("patina.log.1")).unwrap(),
        "preserved-gen-1\n"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("patina.log.2")).unwrap(),
        "preserved-gen-2\n"
    );
}

#[test]
fn export_and_share_helpers_preserve_destinations_and_retention() {
    let tmp = tempfile::tempdir().unwrap();

    // Missing data_dir must never be recreated by panic persistence.
    let missing_root = tmp.path().join("absent-during-restore");
    assert!(write_last_panic_atomic(&missing_root, b"panic").is_err());
    assert!(!missing_root.exists());

    // Staging 6 share files within retention keeps all 6 intact.
    let mut staged = Vec::new();
    for i in 0..6 {
        let path = stage_share_file_in(
            tmp.path(),
            "patina-diagnostics-20261128-000000.txt",
            format!("synthetic-share-{i}").as_bytes(),
        )
        .unwrap();
        staged.push(path);
    }
    for (i, path) in staged.iter().enumerate() {
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            format!("synthetic-share-{i}")
        );
    }

    // Saving into a read-only parent where sibling staging fails must NOT truncate
    // an existing file at `destination`.
    let existing = tmp.path().join("existing-report.txt");
    fs::write(&existing, b"original-important-data").unwrap();
    let bad_dest = tmp.path().join("missing-parent-dir").join("report.txt");
    assert!(write_file_atomic_or_direct(&bad_dest, b"new").is_err());
    assert_eq!(
        fs::read_to_string(&existing).unwrap(),
        "original-important-data"
    );

    // Apple os_log chunking splits long messages on line boundaries and UTF-8 char boundaries.
    assert_eq!(
        os_log_parts("line1\nline2\nline3", 8),
        vec!["line1", "line2", "line3"]
    );
    assert_eq!(os_log_parts("ééééé", 4), vec!["éé", "éé", "é"]);
}

#[test]
fn rotating_writer_seeds_large_startup_snapshot_and_joins_deterministically() {
    let tmp = tempfile::tempdir().unwrap();
    let inner = Arc::new(FakeInnerLogger {
        records: Mutex::new(Vec::new()),
        flushes: AtomicUsize::new(0),
        max_level: log::Level::Info,
    });
    let logger = DiagnosticLogger::with_inner(inner, || Duration::from_millis(10));

    // Buffer 1,500 startup lines (> 1,024 runtime burst queue capacity) before attach.
    for idx in 0..1_500 {
        log::Log::log(
            &logger,
            &log::Record::builder()
                .args(format_args!("startup-seed-{idx:04}"))
                .level(log::Level::Info)
                .target("iptv_boot")
                .build(),
        );
    }

    let writer = Arc::new(
        RotatingWriter::spawn_with_capacity(tmp.path().to_path_buf(), ROTATE_BYTES, 8).unwrap(),
    );
    logger.attach_file_sink(writer.clone());
    for idx in 0..20 {
        writer.write_line(&format!("+0.{idx:03}s INFO  iptv_bg: burst-{idx}\n"));
    }
    logger.close_file_sink();

    let active = fs::read_to_string(tmp.path().join("patina.log")).unwrap();
    assert!(active.contains("startup-seed-0000"), "{active}");
    assert!(active.contains("startup-seed-1499"), "{active}");
    assert!(active.contains("burst-0"), "{active}");
    assert!(
        active.contains("burst-19") || active.contains("[dropped:"),
        "{active}"
    );
}

#[test]
fn panic_hook_persists_redacted_last_panic_and_next_launch_reports_it_once() {
    const CHILD_ENV: &str = "IPTV_DIAG_PANIC_HOOK_CHILD";
    const DIR_ENV: &str = "IPTV_DIAG_PANIC_DIR";

    if let Some(mode) = std::env::var_os(CHILD_ENV) {
        let data_dir = std::path::PathBuf::from(std::env::var_os(DIR_ENV).unwrap());
        super::install();
        if mode == "unowned_panic" {
            // Without `set_panic_dir`, an unowned / rejected instance must not write.
            panic!("unowned instance panic");
        }
        super::set_panic_dir(&data_dir);
        if mode == "panic" {
            let secret_path = data_dir.join("playlists/generated.m3u");
            panic!(
                "synthetic crash opening {} for https://syn_u:syn_p@provider.example.test/live.ts",
                secret_path.display()
            );
        } else if mode == "next_run" {
            super::activate_storage(&data_dir);
            super::shutdown();
            let exported = super::export(&sample_header());
            assert!(
                exported.contains("[last-panic]"),
                "export missing [last-panic]: {exported}"
            );
            assert!(
                exported.contains("previous run ended in a panic"),
                "next run did not log previous panic warning: {exported}"
            );
            assert!(
                read_last_panic_in(&data_dir).is_some(),
                "last-panic.txt must be kept for subsequent exports"
            );
            return;
        } else if mode == "third_run" {
            super::activate_storage(&data_dir);
            super::shutdown();
            let exported = super::export(&sample_header());
            assert!(
                exported.contains("[last-panic]"),
                "third run export must still retain [last-panic]: {exported}"
            );
            assert!(
                !exported.contains("previous run ended in a panic"),
                "third run must not repeat the previous-run crash warning: {exported}"
            );
            return;
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let exe = std::env::current_exe().unwrap();
    let test_name = "host::diagnostics::tests::panic_hook_persists_redacted_last_panic_and_next_launch_reports_it_once";

    // Unowned instance panic must leave no `last-panic.txt` in `HOME` or `temp`.
    let unowned = std::process::Command::new(&exe)
        .args(["--exact", test_name, "--nocapture"])
        .env(CHILD_ENV, "unowned_panic")
        .env(DIR_ENV, temp.path())
        .env("HOME", temp.path())
        .output()
        .unwrap();
    assert!(!unowned.status.success());
    assert!(read_last_panic_in(temp.path()).is_none());

    let crash = std::process::Command::new(&exe)
        .args(["--exact", test_name, "--nocapture"])
        .env(CHILD_ENV, "panic")
        .env(DIR_ENV, temp.path())
        .env("RUST_BACKTRACE", "1")
        .output()
        .unwrap();
    assert!(
        !crash.status.success(),
        "chained default panic hook must still fail the panicking process"
    );

    let persisted = read_last_panic_in(temp.path()).expect("last-panic.txt written atomically");
    assert!(persisted.contains("app=v"), "{persisted}");
    assert!(persisted.contains("thread="), "{persisted}");
    assert!(
        persisted.contains("location=crates/iptv-app/src/host/diagnostics/tests.rs:"),
        "{persisted}"
    );
    assert!(persisted.contains("[backtrace]"), "{persisted}");
    for forbidden in [
        "provider.example.test",
        "syn_u",
        "syn_p",
        "https://",
        &temp.path().to_string_lossy(),
    ] {
        assert!(
            !persisted.contains(forbidden),
            "unredacted {forbidden:?} in last-panic.txt:\n{persisted}"
        );
    }
    let summary = summarize_previous_panic(&persisted, &[]);
    assert!(summary.contains("location=crates/iptv-app/"), "{summary}");

    // Verify atomic overwrite replaces an older panic cleanly without `.tmp` leftovers.
    write_last_panic_atomic(temp.path(), persisted.as_bytes()).unwrap();
    let leftovers: Vec<_> = fs::read_dir(temp.path().join("diagnostics"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".tmp."))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files left behind: {leftovers:?}"
    );

    for phase in ["next_run", "third_run"] {
        let out = std::process::Command::new(&exe)
            .args(["--exact", test_name, "--nocapture"])
            .env(CHILD_ENV, phase)
            .env(DIR_ENV, temp.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{phase} failed:\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

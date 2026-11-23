use super::*;

#[test]
fn the_initialized_provider_exports_keyed_data_on_new_connections() {
    initialize().expect("initialize SQLite and SQLCipher");
    initialize().expect("repeat initialization");

    let conn = Connection::open_in_memory().expect("open a new database after initialization");
    conn.execute_batch(
        "PRAGMA key = \"x'7171717171717171717171717171717171717171717171717171717171717171'\";
         CREATE TABLE generated (value INTEGER);
         INSERT INTO generated VALUES (42);
         ATTACH DATABASE ':memory:' AS generated_export KEY '';
         SELECT sqlcipher_export('generated_export');",
    )
    .expect("export with the registered SQLCipher auto-extension");
    let stored: i64 = conn
        .query_row("SELECT value FROM generated_export.generated", [], |row| {
            row.get(0)
        })
        .expect("read the exported row");
    assert_eq!(stored, 42);
}

#[test]
fn delayed_export_registration_is_polled_until_visible() {
    let ready = OnceLock::new();
    let mut observations = [false, false, true].into_iter();
    wait_for_export(
        &ready,
        || {
            assert_eq!(ready.get(), None, "no probe has seen the export yet");
            Ok(observations.next().expect("at most three probes"))
        },
        Instant::now() + EXPORT_WAIT,
    )
    .expect("export is available after the third probe");
    assert_eq!(observations.next(), None);
    assert_eq!(ready.get(), Some(&()));
}

#[test]
fn the_deadline_returns_busy_when_export_registration_is_missing() {
    let ready = OnceLock::new();
    let error =
        wait_for_export(&ready, || Ok(false), Instant::now()).expect_err("registration timed out");
    let rusqlite::Error::SqliteFailure(code, Some(message)) = error else {
        panic!("expected a SQLite busy error with a diagnostic, got {error:?}");
    };
    assert_eq!(code.extended_code, ffi::SQLITE_BUSY);
    assert!(message.contains("SQLCipher export"));
    assert_eq!(ready.get(), None, "timeout leaves initialization retryable");
    wait_for_export(&ready, || Ok(true), Instant::now())
        .expect("registration found on the next attempt");
    assert_eq!(ready.get(), Some(&()));
}

#[test]
fn a_probe_failure_retains_the_extended_sqlite_status() {
    let ready = OnceLock::new();
    let error = wait_for_export(
        &ready,
        || {
            check(ffi::SQLITE_IOERR_READ)?;
            Ok(false)
        },
        Instant::now(),
    )
    .expect_err("SQLCipher readiness probe failed");
    assert_eq!(
        error.sqlite_error().map(|code| code.extended_code),
        Some(ffi::SQLITE_IOERR_READ)
    );
    assert_eq!(ready.get(), None);
}

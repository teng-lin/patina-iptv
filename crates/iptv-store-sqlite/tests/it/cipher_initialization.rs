//! Owned readiness against foreign native initialization, without a fake provider.
use crate::initialization_support;
use rusqlite::ffi;

// Pinned SQLCipher 4.20 declares this exact C ABI in sqlcipher.c. The
// opaque provider is only observed, never dereferenced or replaced.
unsafe extern "C" {
    fn sqlcipher_get_provider() -> *const std::ffi::c_void;
}

#[test]
fn native_initialization_finishes_before_concurrent_key_attachment() {
    const CHILD: &str = "IPTV_SQLCIPHER_NATIVE_INIT_CHILD";
    if std::env::var_os(CHILD).is_none() {
        for _ in 0..16 {
            let output = initialization_support::child(
                "cipher_initialization::native_initialization_finishes_before_concurrent_key_attachment",
                CHILD,
                std::time::Duration::from_secs(20),
            )
            .expect("native initialization child could not complete");
            assert!(
                output.status.success(),
                "native concurrent initialization failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        return;
    }

    const THREADS: usize = 32;
    let begin = std::sync::Barrier::new(THREADS);
    std::thread::scope(|threads| {
        for index in 0..THREADS {
            let begin = &begin;
            threads.spawn(move || {
                begin.wait();
                if index % 3 == 0 {
                    // SAFETY: a foreign caller may initialize SQLite concurrently;
                    // it never changes callbacks or shuts the library down.
                    assert_eq!(unsafe { ffi::sqlite3_initialize() }, ffi::SQLITE_OK);
                    return;
                }
                iptv_sqlcipher_init::initialize().unwrap();
                // SAFETY: the owned adapter promises that its pinned SQLCipher
                // provider is ready. This observes, never dereferences, its pointer.
                let provider = unsafe { sqlcipher_get_provider() };
                assert!(
                    !provider.is_null(),
                    "owned initialization returned before the cipher provider was ready"
                );
                let mut connection = std::ptr::null_mut();
                // SAFETY: the path and VFS pointers live through the call; SQLite
                // initializes the output handle, which this thread owns exclusively.
                let opened = unsafe {
                    ffi::sqlite3_open_v2(
                        c":memory:".as_ptr(),
                        &mut connection,
                        ffi::SQLITE_OPEN_READWRITE
                            | ffi::SQLITE_OPEN_CREATE
                            | ffi::SQLITE_OPEN_NOMUTEX,
                        std::ptr::null(),
                    )
                };
                assert_eq!(opened, ffi::SQLITE_OK);
                assert!(!connection.is_null());
                // Synthetic raw-key syntax avoids password derivation work; no
                // key or real provider value is read from the environment.
                let key = c"x'6060606060606060606060606060606060606060606060606060606060606060'";
                // SAFETY: connection is live and confined to this thread. The key
                // buffer is valid for its declared length; SQLCipher copies it.
                let keyed = unsafe {
                    ffi::sqlite3_key(
                        connection,
                        key.as_ptr().cast(),
                        i32::try_from(key.to_bytes().len()).unwrap(),
                    )
                };
                let exported = if keyed == ffi::SQLITE_OK {
                    // SAFETY: the statement is static, references only synthetic
                    // in-memory databases, and installs no callback. It exercises
                    // the real auto-extension used by legacy migration/backup.
                    unsafe {
                        ffi::sqlite3_exec(
                            connection,
                            c"CREATE TABLE generated(value); \
                              INSERT INTO generated VALUES(1); \
                              ATTACH DATABASE ':memory:' AS generated_export KEY ''; \
                              SELECT sqlcipher_export('generated_export');"
                                .as_ptr(),
                            None,
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                        )
                    }
                } else {
                    keyed
                };
                // SAFETY: no statements or other references retain this connection.
                let closed = unsafe { ffi::sqlite3_close(connection) };
                assert_eq!(keyed, ffi::SQLITE_OK, "SQLCipher was not ready after open");
                assert_eq!(
                    exported,
                    ffi::SQLITE_OK,
                    "SQLCipher export was not registered"
                );
                assert_eq!(closed, ffi::SQLITE_OK);
            });
        }
    });
}

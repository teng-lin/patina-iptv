//! Android host unwraps the database key with its non-exportable Keystore key.
use crate::runtime::{Error, model::DatabaseKey};
use jni::{
    JNIEnv,
    objects::{JByteArray, JClass},
    sys::jboolean,
};
use std::sync::OnceLock;
use zeroize::Zeroizing;

static KEY: OnceLock<Zeroizing<[u8; 32]>> = OnceLock::new();

pub(super) fn database_key() -> Result<DatabaseKey, Error> {
    KEY.get()
        .map(|key| DatabaseKey::from_bytes(**key))
        .ok_or(Error::unavailable("Android Keystore is unavailable"))
}

// SAFETY: Android calls this symbol using the matching static JNI declaration;
// all Java handles and array bounds are checked through jni's safe interface.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_hantekllc_iptvrs_host_CredentialKey_installDatabaseKey(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
    bytes: JByteArray<'_>,
) -> jboolean {
    // A panic must not unwind out of an `extern "system"` function into the
    // JVM (the process aborts): it counts as "not installed". Its message
    // is not logged.
    let installed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> jboolean {
        let Ok(raw) = env.convert_byte_array(&bytes) else {
            return 0;
        };
        let raw = Zeroizing::new(raw);
        let Ok(key) = <[u8; 32]>::try_from(raw.as_slice()) else {
            return 0;
        };
        match KEY.set(Zeroizing::new(key)) {
            Ok(()) => 1,
            // Activity recreation must install the same key, never silently accept
            // changed persistent credentials while the old key remains in memory.
            Err(candidate) => u8::from(KEY.get().is_some_and(|existing| **existing == *candidate)),
        }
    }));
    installed.unwrap_or_else(|_| {
        log::error!("iptv credential key: installing the database key panicked");
        0
    })
}

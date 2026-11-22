//! The device services an application on the real runtime takes from the
//! host besides the network: secure randomness and the local time zone.
//!
//! Imports (module `host`): `random(ptr, len) -> u32` fills `len` bytes
//! with the Worker's `crypto.getRandomValues` and returns 1 (0 when it
//! could not); `utc_offset_minutes(unix_ms: f64) -> f64`, local time minus
//! UTC at that instant (`-new Date(ms).getTimezoneOffset()`). No 64-bit
//! integer crosses: times are `f64` milliseconds.

/// Fills `buf` with cryptographically secure random bytes.
pub fn fill_random(buf: &mut [u8]) -> std::io::Result<()> {
    if imports::random(buf) {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "the host supplied no secure randomness",
        ))
    }
}

/// Local time minus UTC at `unix_seconds`, in minutes.
pub fn utc_offset_minutes(unix_seconds: i64) -> i32 {
    let minutes = imports::utc_offset_minutes(unix_seconds as f64 * 1000.0);
    // Real offsets are within ±18 hours; anything else is no answer.
    if minutes.is_finite() && minutes.abs() <= 18.0 * 60.0 {
        minutes.round() as i32
    } else {
        0
    }
}

#[cfg(target_arch = "wasm32")]
mod imports {
    #[link(wasm_import_module = "host")]
    unsafe extern "C" {
        #[link_name = "random"]
        fn host_random(ptr: *mut u8, len: usize) -> u32;
        #[link_name = "utc_offset_minutes"]
        fn host_utc_offset_minutes(unix_ms: f64) -> f64;
    }

    pub(super) fn random(buf: &mut [u8]) -> bool {
        // getRandomValues takes at most 65,536 bytes a call.
        buf.chunks_mut(65_536).all(|part| {
            // SAFETY: the host writes `len` bytes at `ptr` before it returns.
            unsafe { host_random(part.as_mut_ptr(), part.len()) == 1 }
        })
    }

    pub(super) fn utc_offset_minutes(unix_ms: f64) -> f64 {
        // SAFETY: a plain value argument; returns a number.
        unsafe { host_utc_offset_minutes(unix_ms) }
    }
}

/// Native builds (tests) have no host: no randomness, UTC.
#[cfg(not(target_arch = "wasm32"))]
mod imports {
    pub(super) fn random(_buf: &mut [u8]) -> bool {
        false
    }
    pub(super) fn utc_offset_minutes(_unix_ms: f64) -> f64 {
        0.0
    }
}

//! Files of the application's own package, read synchronously: what a
//! build keeps out of its module to make it smaller (an interface
//! language's messages), read when it is first needed.
//!
//! Imports (module `host`): `package_file_size(ptr, len) -> f64` reads the
//! package file at the UTF-8 relative path (`ptr`, `len`) and keeps its
//! bytes until the next call, returning their length (-1: no such file);
//! `package_file_read(ptr, len) -> u32` copies them to `ptr` (which has
//! their length) and drops the host's copy. The Worker reads the file with
//! a synchronous request: package files are local and small, and only a
//! path inside the package is asked for.

/// The package file at `path` (relative, inside the package), or `None`
/// when the package has none or the host could not read it.
pub fn package_file(path: &str) -> Option<Vec<u8>> {
    if path.starts_with('/') || path.split('/').any(|part| part == ".." || part.is_empty()) {
        return None;
    }
    let size = imports::size(path);
    if !(size >= 0.0 && size.fract() == 0.0 && size <= u32::MAX as f64) {
        return None;
    }
    let mut bytes = vec![0; size as usize];
    (imports::read(&mut bytes) == bytes.len()).then_some(bytes)
}

#[cfg(target_arch = "wasm32")]
mod imports {
    #[link(wasm_import_module = "host")]
    unsafe extern "C" {
        fn package_file_size(ptr: *const u8, len: usize) -> f64;
        fn package_file_read(ptr: *mut u8, len: usize) -> u32;
    }

    pub(super) fn size(path: &str) -> f64 {
        // SAFETY: the host reads `len` bytes at `ptr` before it returns.
        unsafe { package_file_size(path.as_ptr(), path.len()) }
    }

    pub(super) fn read(buf: &mut [u8]) -> usize {
        // SAFETY: the host writes at most `len` bytes at `ptr` before it
        // returns and keeps no reference.
        unsafe { package_file_read(buf.as_mut_ptr(), buf.len()) as usize }
    }
}

/// Native builds (tests) have no package.
#[cfg(not(target_arch = "wasm32"))]
mod imports {
    pub(super) fn size(_path: &str) -> f64 {
        -1.0
    }
    pub(super) fn read(_buf: &mut [u8]) -> usize {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::package_file;

    #[test]
    fn paths_outside_the_package_are_not_asked_for() {
        for path in ["/etc/passwd", "../x", "a/../../b", "a//b", ""] {
            assert_eq!(package_file(path), None, "{path}");
        }
        assert_eq!(package_file("i18n/fr.tsv"), None, "no package natively");
    }
}

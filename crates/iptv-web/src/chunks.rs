//! Chunked database storage in the host's IndexedDB: what a browser store
//! (`iptv-store-sqlite`'s memory files) is restored from at launch and
//! saved to after each commit, in chunks of the store's size.
//!
//! The host loads every stored chunk before the module starts, so the
//! store's open, which cannot wait, reads them synchronously
//! ([`stored_size`], [`read_stored`]); [`release_stored`] frees the host's
//! copy once the store has its own. A save ([`save`]) hands the host one
//! commit's changed chunks and the file's size; the host writes them, the
//! size and the removal of chunks past the end in one transaction, and
//! answers once it committed or failed.
//!
//! Imports (module `host`): `chunks_stored_size() -> f64` (-1: none stored,
//! -2: what is stored could not be read),
//! `chunks_stored_read(index, ptr, len) -> u32` (bytes copied),
//! `chunks_stored_release()`, `chunks_put(save, index, ptr, len)` (copied
//! before it returns), `chunks_commit(save, size: f64)`. Export:
//! `on_chunks_saved(save, ok)`. Sizes cross as `f64` (Chromium 69 has no
//! 64-bit integers at the boundary); a database is far below 2^53 bytes.

/// The size of the stored database: `None` when none is stored, an error
/// when the host could not read what is stored (opening over it would
/// lose it).
pub fn stored_size() -> Result<Option<u64>, Unreadable> {
    let size = imports::stored_size();
    if size == -1.0 {
        Ok(None)
    } else if size >= 0.0 && size.fract() == 0.0 && size < 9_007_199_254_740_992.0 {
        Ok(Some(size as u64))
    } else {
        Err(Unreadable)
    }
}

/// The host could not read the stored database (IndexedDB failed, or a
/// chunk was missing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unreadable;

/// Copies stored chunk `index` into `buf`, which has the chunk's length;
/// false when the host has no such chunk or another length.
pub fn read_stored(index: u32, buf: &mut [u8]) -> bool {
    imports::stored_read(index, buf) == buf.len()
}

/// The store holds what was restored: the host drops its copy.
pub fn release_stored() {
    imports::stored_release();
}

/// Hands the host one take of changed chunks and the database's `size`,
/// copied before this returns. The future ends true once the host's
/// transaction committed, false when it failed (offer the chunks again).
pub fn save<'a>(
    chunks: impl Iterator<Item = (u32, &'a [u8])>,
    size: u64,
) -> impl std::future::Future<Output = bool> + 'static {
    let saving = crate::exec::Saving::new();
    for (index, bytes) in chunks {
        imports::put(saving.id(), index, bytes);
    }
    imports::commit(saving.id(), size as f64);
    saving.answer()
}

#[cfg(target_arch = "wasm32")]
mod imports {
    #[link(wasm_import_module = "host")]
    unsafe extern "C" {
        fn chunks_stored_size() -> f64;
        fn chunks_stored_read(index: u32, ptr: *mut u8, len: usize) -> u32;
        fn chunks_stored_release();
        fn chunks_put(save: u32, index: u32, ptr: *const u8, len: usize);
        fn chunks_commit(save: u32, size: f64);
    }

    pub(super) fn stored_size() -> f64 {
        // SAFETY: no arguments; returns a number.
        unsafe { chunks_stored_size() }
    }

    pub(super) fn stored_read(index: u32, buf: &mut [u8]) -> usize {
        // SAFETY: the host writes at most `len` bytes at `ptr` before it
        // returns and keeps no reference.
        unsafe { chunks_stored_read(index, buf.as_mut_ptr(), buf.len()) as usize }
    }

    pub(super) fn stored_release() {
        // SAFETY: no arguments.
        unsafe { chunks_stored_release() }
    }

    pub(super) fn put(save: u32, index: u32, bytes: &[u8]) {
        // SAFETY: the host copies `len` bytes at `ptr` before it returns.
        unsafe { chunks_put(save, index, bytes.as_ptr(), bytes.len()) }
    }

    pub(super) fn commit(save: u32, size: f64) {
        // SAFETY: plain value arguments.
        unsafe { chunks_commit(save, size) }
    }
}

/// Native builds (tests) have no host storage: nothing is stored, and a
/// save fails.
#[cfg(not(target_arch = "wasm32"))]
mod imports {
    pub(super) fn stored_size() -> f64 {
        -1.0
    }
    pub(super) fn stored_read(_index: u32, _buf: &mut [u8]) -> usize {
        0
    }
    pub(super) fn stored_release() {}
    pub(super) fn put(_save: u32, _index: u32, _bytes: &[u8]) {}
    pub(super) fn commit(save: u32, _size: f64) {
        crate::exec::deliver_saved(save, false);
    }
}

//! The app's small files (settings, each playlist's last channel) in the
//! host's IndexedDB: what a browser application's file port restores at
//! launch and saves after each write.
//!
//! The host reads every stored file before the module starts, as it reads
//! the store's chunks, so the app's startup, which cannot wait, finds them
//! at hand ([`StoredFiles::restore`]), and they are kept here by name for
//! the session: reads answer at once and see every write handed over. A
//! write hands the host one file's new content, or its removal; the host
//! saves it in its own transaction, after every save handed over before
//! it, and answers once it committed or failed.
//!
//! Imports (module `host`): `files_stored_size() -> i32` (bytes of the
//! stored files, packed: per file a little-endian `u32` name length, the
//! name in UTF-8, a `u32` content length, the content),
//! `files_stored_read(ptr, len) -> u32` (bytes copied; the host then drops
//! its copy), `files_put(save, name_ptr, name_len, ptr, len, present)`
//! (`present` 0 removes the file; copied before it returns). Export:
//! `on_chunks_saved(save, ok)`, which answers chunk saves too.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The app's small files: what the host had stored at launch and every
/// write since.
pub struct StoredFiles {
    files: Arc<Mutex<Files>>,
}

#[derive(Default)]
struct File {
    saved: Option<Vec<u8>>,
    saved_write: u64,
    next_write: u64,
    pending: BTreeMap<u64, Option<Vec<u8>>>,
}

type Files = HashMap<String, File>;

fn lock(files: &Mutex<Files>) -> MutexGuard<'_, Files> {
    files.lock().unwrap_or_else(PoisonError::into_inner)
}

type Save = Pin<Box<dyn Future<Output = bool>>>;

impl StoredFiles {
    /// The files the host had stored at launch, or `None` if storage
    /// was unreadable. Call once: the host drops its copy.
    pub fn restore() -> Option<Self> {
        stored().map(Self::with)
    }

    fn with(stored: Vec<(String, Vec<u8>)>) -> Self {
        Self {
            files: Arc::new(Mutex::new(
                stored
                    .into_iter()
                    .map(|(name, bytes)| {
                        let file = File {
                            saved: Some(bytes),
                            ..File::default()
                        };
                        (name, file)
                    })
                    .collect(),
            )),
        }
    }

    /// What `name` holds; `None` for no file.
    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        let files = lock(&self.files);
        let file = files.get(name)?;
        match file.pending.last_key_value() {
            Some((_, content)) => content.clone(),
            None => file.saved.clone(),
        }
    }

    /// Replaces `name` with `content` (`None` removes it) and hands the
    /// host the write. Reads answer `content` from now on; the future ends
    /// true once the host saved it, false when it did not, and reads then
    /// fall back to the latest pending or successfully stored content.
    pub fn write(
        &self,
        name: &str,
        content: Option<Vec<u8>>,
    ) -> impl Future<Output = bool> + 'static {
        let save = self.write_with(name, content, |name, content| Box::pin(save(name, content)));
        // Playback intentionally drops the reply. The cache still needs
        // to settle that write when IndexedDB answers.
        let (reply, answer) = futures_channel::oneshot::channel();
        crate::exec::spawn(async move {
            let _ = reply.send(save.await);
        });
        async move { answer.await.unwrap_or(false) }
    }

    fn write_with(
        &self,
        name: &str,
        content: Option<Vec<u8>>,
        save: impl FnOnce(&str, Option<&[u8]>) -> Save,
    ) -> impl Future<Output = bool> + 'static {
        let name = name.to_owned();
        let write = {
            let mut files = lock(&self.files);
            let file = files.entry(name.clone()).or_default();
            file.next_write += 1;
            file.pending.insert(file.next_write, content.clone());
            file.next_write
        };
        let saved = save(&name, content.as_deref());
        let files = self.files.clone();
        async move {
            let ok = saved.await;
            let mut files = lock(&files);
            let file = files.get_mut(&name).expect("a write registers its file");
            file.pending.remove(&write);
            if ok && write > file.saved_write {
                file.saved = content;
                file.saved_write = write;
                file.pending.retain(|&pending, _| pending > write);
            }
            if !ok {
                log::warn!("iptv files: the TV did not save a small file");
            }
            ok
        }
    }
}

/// The files the host had stored at launch, by name.
fn stored() -> Option<Vec<(String, Vec<u8>)>> {
    let size = usize::try_from(imports::stored_size()).ok()?;
    if size == 0 {
        return Some(Vec::new());
    }
    let mut packed = vec![0; size];
    if imports::stored_read(&mut packed) != size {
        log::warn!("iptv files: the host did not hand over its stored files");
        return None;
    }
    Some(unpack(&packed))
}

/// The files in `packed` (the host's layout); a malformed tail is dropped.
fn unpack(mut packed: &[u8]) -> Vec<(String, Vec<u8>)> {
    fn take<'a>(packed: &mut &'a [u8]) -> Option<&'a [u8]> {
        let (len, rest) = packed.split_first_chunk::<4>()?;
        let len = u32::from_le_bytes(*len) as usize;
        if rest.len() < len {
            return None;
        }
        let (bytes, rest) = rest.split_at(len);
        *packed = rest;
        Some(bytes)
    }
    let mut files = Vec::new();
    while !packed.is_empty() {
        let (Some(name), Some(content)) = (take(&mut packed), take(&mut packed)) else {
            log::warn!("iptv files: the stored files end early");
            break;
        };
        let Ok(name) = String::from_utf8(name.to_vec()) else {
            continue;
        };
        files.push((name, content.to_vec()));
    }
    files
}

/// Hands the host `name`'s new `content` (`None` removes the file), copied
/// before this returns. The future ends true once the host's transaction
/// committed, false when it failed.
fn save(name: &str, content: Option<&[u8]>) -> impl Future<Output = bool> + 'static {
    let saving = crate::exec::Saving::new();
    imports::put(saving.id(), name, content);
    saving.answer()
}

#[cfg(target_arch = "wasm32")]
mod imports {
    #[link(wasm_import_module = "host")]
    unsafe extern "C" {
        fn files_stored_size() -> i32;
        fn files_stored_read(ptr: *mut u8, len: usize) -> u32;
        fn files_put(
            save: u32,
            name_ptr: *const u8,
            name_len: usize,
            ptr: *const u8,
            len: usize,
            present: u32,
        );
    }

    pub(super) fn stored_size() -> i32 {
        // SAFETY: no arguments; returns a number.
        unsafe { files_stored_size() }
    }

    pub(super) fn stored_read(buf: &mut [u8]) -> usize {
        // SAFETY: the host writes at most `len` bytes at `ptr` before it
        // returns and keeps no reference.
        unsafe { files_stored_read(buf.as_mut_ptr(), buf.len()) as usize }
    }

    pub(super) fn put(save: u32, name: &str, content: Option<&[u8]>) {
        let bytes = content.unwrap_or_default();
        // SAFETY: the host copies both ranges before it returns.
        unsafe {
            files_put(
                save,
                name.as_ptr(),
                name.len(),
                bytes.as_ptr(),
                bytes.len(),
                u32::from(content.is_some()),
            )
        }
    }
}

/// Native builds (tests) have no host storage: nothing is stored, and a
/// save fails.
#[cfg(not(target_arch = "wasm32"))]
mod imports {
    pub(super) fn stored_size() -> i32 {
        0
    }
    pub(super) fn stored_read(_buf: &mut [u8]) -> usize {
        0
    }
    pub(super) fn put(save: u32, _name: &str, _content: Option<&[u8]>) {
        crate::exec::deliver_saved(save, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now<T>(future: impl Future<Output = T>) -> T {
        let mut future = std::pin::pin!(future);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match future.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(value) => value,
            std::task::Poll::Pending => panic!("the synthetic save has answered"),
        }
    }

    fn answer(ok: bool) -> impl FnOnce(&str, Option<&[u8]>) -> Save {
        move |_, _| Box::pin(async move { ok })
    }

    #[test]
    fn reads_see_writes_and_a_failed_save_restores_the_stored_content() {
        let name = "/synthetic/settings.json";
        let files = StoredFiles::with(vec![(name.to_owned(), b"{}".to_vec())]);
        assert_eq!(files.read(name).as_deref(), Some(&b"{}"[..]));

        let saved = files.write_with(name, Some(b"1".to_vec()), answer(true));
        assert_eq!(files.read(name).as_deref(), Some(&b"1"[..]));
        assert!(now(saved));

        let failed = files.write_with(name, Some(b"2".to_vec()), answer(false));
        assert_eq!(
            files.read(name).as_deref(),
            Some(&b"2"[..]),
            "read your writes"
        );
        assert!(!now(failed));
        assert_eq!(
            files.read(name).as_deref(),
            Some(&b"1"[..]),
            "the saved content"
        );

        // A removal that fails after a newer write leaves the newer content.
        let late = std::rc::Rc::new(std::cell::Cell::new(None::<bool>));
        let answered = late.clone();
        let removed = files.write_with(name, None, move |_, _| {
            Box::pin(std::future::poll_fn(move |_| match answered.get() {
                Some(ok) => std::task::Poll::Ready(ok),
                None => std::task::Poll::Pending,
            }))
        });
        assert_eq!(files.read(name), None);
        let newer = files.write_with(name, Some(b"3".to_vec()), answer(true));
        late.set(Some(false));
        assert!(!now(removed));
        assert!(now(newer));
        assert_eq!(files.read(name).as_deref(), Some(&b"3"[..]));
    }

    #[test]
    fn two_failed_writes_fall_back_to_the_last_committed_file() {
        let name = "/synthetic/settings.json";
        let files = StoredFiles::with(vec![(name.to_owned(), b"{}".to_vec())]);
        let older = files.write_with(name, Some(b"1".to_vec()), answer(false));
        let newer = files.write_with(name, Some(b"2".to_vec()), answer(false));
        assert!(!now(older));
        assert_eq!(files.read(name).as_deref(), Some(&b"2"[..]));
        assert!(!now(newer));
        assert_eq!(files.read(name).as_deref(), Some(&b"{}"[..]));
    }

    #[test]
    fn dropping_the_reply_still_settles_a_failed_write() {
        let files = StoredFiles::restore().unwrap();
        let name = "/synthetic/last_channel";
        drop(files.write(name, Some(b"channel".to_vec())));
        assert_eq!(files.read(name).as_deref(), Some(&b"channel"[..]));
        crate::exec::poll();
        assert_eq!(files.read(name), None);
    }

    fn pack(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut packed = Vec::new();
        for (name, content) in files {
            packed.extend_from_slice(&(name.len() as u32).to_le_bytes());
            packed.extend_from_slice(name.as_bytes());
            packed.extend_from_slice(&(content.len() as u32).to_le_bytes());
            packed.extend_from_slice(content);
        }
        packed
    }

    #[test]
    fn stored_files_unpack_in_the_hosts_layout() {
        let packed = pack(&[
            ("/synthetic/settings.json", b"{}"),
            ("/synthetic/empty", b""),
        ]);
        assert_eq!(
            unpack(&packed),
            vec![
                ("/synthetic/settings.json".to_owned(), b"{}".to_vec()),
                ("/synthetic/empty".to_owned(), Vec::new()),
            ]
        );
        // A truncated tail keeps the whole files before it.
        assert_eq!(unpack(&packed[..packed.len() - 1]).len(), 1);
    }
}

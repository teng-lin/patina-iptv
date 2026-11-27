//! Retain verified package guards across About view lifetimes. One runtime owns
//! at most one pending update; replacement, cancellation and shutdown drop it.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tempfile::TempPath;

use super::ReleaseAsset;

pub(crate) struct Pending {
    asset: ReleaseAsset,
    ready: Ready,
}

pub(crate) struct Progress {
    pub(crate) received: AtomicU64,
    pub(crate) total: u64,
}

pub(crate) struct Active {
    pub(crate) progress: Arc<Progress>,
    _cancel: tokio::sync::oneshot::Sender<()>,
}

#[cfg(any(target_os = "linux", test))]
struct FileCommitGuard<'a>(&'a std::cell::Cell<usize>);

#[cfg(any(target_os = "linux", test))]
impl Drop for FileCommitGuard<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

/// Clears only its own operation, even if an older request finishes after its
/// replacement. Dropping a caller also cancels its worker and partial file.
pub(super) struct DownloadGuard<'a> {
    owner: &'a crate::Runtime,
    progress: Arc<Progress>,
}

impl DownloadGuard<'_> {
    pub(super) fn is_current(&self) -> bool {
        self.owner
            .lifecycle
            .app_updates
            .download
            .borrow()
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(&active.progress, &self.progress))
    }
}

impl Drop for DownloadGuard<'_> {
    fn drop(&mut self) {
        let mut active = self.owner.lifecycle.app_updates.download.borrow_mut();
        if active
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(&active.progress, &self.progress))
        {
            active.take();
        }
    }
}

enum Ready {
    Download(TempPath),
    #[cfg(target_os = "linux")]
    AppImage(super::appimage::StagedImage),
    #[cfg(windows)]
    Windows(super::windows::PendingSetup),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Download,
    #[cfg(target_os = "linux")]
    AppImage,
    #[cfg(windows)]
    Windows,
}

impl Pending {
    fn kind(&self) -> Kind {
        match &self.ready {
            Ready::Download(_) => Kind::Download,
            #[cfg(target_os = "linux")]
            Ready::AppImage(_) => Kind::AppImage,
            #[cfg(windows)]
            Ready::Windows(_) => Kind::Windows,
        }
    }
}

impl crate::Runtime {
    /// Enter before submitting an irreversible AppImage file operation. The
    /// caller keeps this borrowed guard until the worker's result is handled.
    #[cfg(any(target_os = "linux", test))]
    pub fn begin_app_update_file_commit(&self) -> impl Drop + '_ {
        self.lifecycle
            .app_updates
            .file_commits
            .set(self.lifecycle.app_updates.file_commits.get() + 1);
        FileCommitGuard(&self.lifecycle.app_updates.file_commits)
    }

    #[cfg(any(target_os = "linux", test))]
    pub fn app_update_file_commit_active(&self) -> bool {
        self.lifecycle.app_updates.file_commits.get() != 0
    }

    pub fn app_update_download_progress(&self) -> Option<(u64, u64)> {
        self.lifecycle
            .app_updates
            .download
            .borrow()
            .as_ref()
            .map(|active| {
                (
                    active.progress.received.load(Ordering::Relaxed),
                    active.progress.total,
                )
            })
    }

    /// Stops the owned network operation. Its worker releases the partial file.
    pub fn cancel_app_update_download(&self) {
        self.lifecycle.app_updates.download.take();
    }

    pub(super) fn begin_update_download(
        &self,
        total: u64,
    ) -> (
        DownloadGuard<'_>,
        Arc<Progress>,
        tokio::sync::oneshot::Receiver<()>,
    ) {
        self.cancel_pending_app_update();
        let progress = Arc::new(Progress {
            received: AtomicU64::new(0),
            total,
        });
        let (send, receive) = tokio::sync::oneshot::channel();
        self.lifecycle.app_updates.download.replace(Some(Active {
            progress: Arc::clone(&progress),
            _cancel: send,
        }));
        (
            DownloadGuard {
                owner: self,
                progress: Arc::clone(&progress),
            },
            progress,
            receive,
        )
    }

    /// Retains the verified bytes without transferring ownership to a view.
    /// A new operation replaces and cleans the previous operation's guard.
    pub fn retain_update_download(&self, asset: ReleaseAsset, path: TempPath) {
        self.lifecycle.app_updates.pending.replace(Some(Pending {
            asset,
            ready: Ready::Download(path),
        }));
    }

    /// Only the exact authenticated asset can reclaim a previously verified file.
    /// A mismatched asset or stage kind leaves the pending operation intact.
    pub fn take_update_download(&self, asset: &ReleaseAsset) -> Option<TempPath> {
        match self.take_pending_update(asset, Kind::Download)?.ready {
            Ready::Download(path) => Some(path),
            #[cfg(any(target_os = "linux", windows))]
            _ => None,
        }
    }

    #[cfg(target_os = "linux")]
    pub fn retain_appimage(&self, asset: ReleaseAsset, image: super::appimage::StagedImage) {
        self.lifecycle.app_updates.pending.replace(Some(Pending {
            asset,
            ready: Ready::AppImage(image),
        }));
    }

    #[cfg(target_os = "linux")]
    pub fn take_appimage(&self, asset: &ReleaseAsset) -> Option<super::appimage::StagedImage> {
        match self.take_pending_update(asset, Kind::AppImage)?.ready {
            Ready::AppImage(image) => Some(image),
            _ => None,
        }
    }

    #[cfg(windows)]
    pub fn retain_windows_setup(&self, asset: ReleaseAsset, setup: super::windows::PendingSetup) {
        self.lifecycle.app_updates.pending.replace(Some(Pending {
            asset,
            ready: Ready::Windows(setup),
        }));
    }

    #[cfg(windows)]
    pub fn take_windows_setup(&self, asset: &ReleaseAsset) -> Option<super::windows::PendingSetup> {
        match self.take_pending_update(asset, Kind::Windows)?.ready {
            Ready::Windows(setup) => Some(setup),
            _ => None,
        }
    }

    pub fn pending_app_update_asset(&self) -> Option<ReleaseAsset> {
        self.lifecycle
            .app_updates
            .pending
            .borrow()
            .as_ref()
            .map(|pending| pending.asset.clone())
    }

    pub fn has_pending_app_update(&self) -> bool {
        self.lifecycle.app_updates.pending.borrow().is_some()
    }

    /// Explicitly cancel the retained update and release its file/installer guard.
    pub fn cancel_pending_app_update(&self) {
        self.lifecycle.app_updates.pending.take();
    }

    fn take_pending_update(&self, asset: &ReleaseAsset, kind: Kind) -> Option<Pending> {
        let mut pending = self.lifecycle.app_updates.pending.borrow_mut();
        let stored = pending.as_ref()?;
        if &stored.asset != asset || stored.kind() != kind {
            return None;
        }
        pending.take()
    }
}

#[cfg(test)]
mod tests;

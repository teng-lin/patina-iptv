//! Owned application state; all types here are independent of the shell.
use iptv_guide::EpgSchedule;
use iptv_import::CancelToken;
use iptv_library::{Playlist, SourceId, SourceLine};
use iptv_store_sqlite::{StoredGuide, StoredSource};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::{Rc, Weak},
};

pub struct ImportWork {
    pub finished: Cell<bool>,
    pub catalog_done: Cell<bool>,
    pub cancel: CancelToken,
    /// Tasks waiting for [`Self::finish`].
    pub waiting: RefCell<Vec<futures_channel::oneshot::Sender<()>>>,
}
impl ImportWork {
    pub fn finished(&self) -> bool {
        self.finished.get()
    }
    pub fn finish(&self) {
        self.finished.set(true);
        for waiting in self.waiting.take() {
            let _ = waiting.send(());
        }
    }
    /// Resolves when the work has finished (at once if it has).
    pub fn until_finished(&self) -> impl std::future::Future<Output = ()> + 'static {
        let wait = (!self.finished.get()).then(|| {
            let (sender, receiver) = futures_channel::oneshot::channel();
            self.waiting.borrow_mut().push(sender);
            receiver
        });
        async move {
            if let Some(wait) = wait {
                let _ = wait.await;
            }
        }
    }
    pub fn playlist_done(&self) {
        self.catalog_done.set(true);
    }
    pub fn cancel_guide(&self) {
        if self.catalog_done.get() {
            self.cancel.cancel();
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct LibrarySnapshot {
    pub playlists: Vec<Playlist>,
    pub details: BTreeMap<SourceId, StoredSource>,
    pub imports: BTreeMap<SourceId, iptv_store_sqlite::ImportStatus>,
    pub active: Option<SourceId>,
}

/// The current single-source schedule can enter the combined catalogue without
/// dropping a portal session, account format, connection limit or loaded guide.
#[derive(Clone)]
pub struct SingleSchedule {
    pub source: SourceId,
    pub schedule: Rc<EpgSchedule>,
    pub limit: Option<u32>,
    pub pending: Option<Rc<ImportWork>>,
    pub revision: Option<Weak<ImportWork>>,
}

#[derive(Default)]
pub struct StatusSnapshot {
    pub library: Option<LibrarySnapshot>,
    pub combined: bool,
    pub enabled: Vec<SourceId>,
    pub guides: BTreeMap<SourceId, StoredGuide>,
    pub lines: BTreeMap<SourceId, Vec<SourceLine>>,
}

/// Organization read atomically for one committed library version.
pub struct OrganizationSnapshot {
    pub organization: iptv_guide::Organization,
    pub lists: Vec<(String, String, Vec<String>)>,
}
impl OrganizationSnapshot {
    pub fn install(self, schedule: &EpgSchedule) {
        schedule.set_organization(self.organization);
        schedule.set_group_lists(self.lists);
    }
}

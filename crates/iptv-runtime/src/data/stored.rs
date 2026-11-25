//! Stored guide jobs, cached status, and on-demand descriptions.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::future::Future;
use std::rc::Rc;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use crate::imports::Msg;
use crate::model::{SourceId, StoredGuide};
use crate::stored::{configs, download_now, with_storage};
use crate::{Error, GuideWork, Runtime};
use iptv_epg::{ChannelKey, UnixTime};
use iptv_import::{CancelToken, EpgSourceConfig};

/// Refresh the stored status and report whether the library snapshot changed.
pub(super) fn load_status(owner: &Rc<Runtime>) -> impl Future<Output = Option<bool>> + use<> {
    owner
        .status_revision
        .set(owner.status_revision.get().wrapping_add(1));
    let expected = owner.status_revision.get();
    let loaded = crate::commands::guide_status_snapshot(owner);
    let owner = owner.clone();
    async move {
        let mut snapshot = loaded.await.ok()?;
        if owner.status_revision.get() != expected {
            return None;
        }
        let changed = snapshot
            .library
            .take()
            .is_some_and(|snapshot| crate::library::install(&owner, snapshot));
        *owner.status.borrow_mut() = snapshot;
        Some(changed)
    }
}

/// Fetch descriptions on the shared worker; a bounded cache makes later renders synchronous.
pub fn description_lookup(
    owner: &Rc<crate::Runtime>,
    source: SourceId,
) -> Option<impl Fn(&ChannelKey, UnixTime) -> Option<String> + 'static> {
    let cache = Rc::new(RefCell::new(
        BTreeMap::<(String, UnixTime), Option<String>>::new(),
    ));
    let owner = Rc::downgrade(owner);
    Some(move |key: &ChannelKey, start| {
        let identity = (key.to_string(), start);
        if let Some(description) = cache.borrow().get(&identity) {
            return description.clone();
        }
        let owner = owner.upgrade()?;
        if cache.borrow().len() >= 128 {
            cache.borrow_mut().pop_first();
        }
        cache.borrow_mut().insert(identity.clone(), None);
        let key = key.clone();
        let read = crate::commands::description(&owner, source, key, start);
        let cache = cache.clone();
        owner.spawn({
            let owner = owner.clone();
            async move {
                match read.await {
                    Ok(description) => {
                        let mut cache = cache.borrow_mut();
                        let Some(entry) = cache.get_mut(&identity) else {
                            return;
                        };
                        *entry = description;
                    }
                    Err(error) => {
                        log::warn!("iptv data: description: {error}");
                        // A transient read failure is not a cached absence. Retry
                        // on the next user-driven lookup without causing a render
                        // loop while the database remains unavailable.
                        cache.borrow_mut().remove(&identity);
                        return;
                    }
                }
                owner.emit(crate::RuntimeEvent::GuideChanged);
            }
        });
        None
    })
}

/// Work canceled by a guide action, until its writer guards unwind.
pub(crate) struct Superseded {
    pub canceled: Option<crate::library::ImportCompletion>,
    pub read: Option<GuideWork>,
}
pub(crate) fn supersede(owner: &Rc<crate::Runtime>, source: SourceId) -> Superseded {
    crate::stored::next_guide_edit(owner, source);
    Superseded {
        canceled: crate::library::supersede_import(owner, source),
        read: owner.interrupt_guide_read(source),
    }
}

pub(super) fn has_enabled_guides(owner: &Runtime, source: SourceId) -> bool {
    owner.status.with(|status| {
        let status = status.borrow();
        match status.lines.get(&source) {
            Some(lines) if !lines.is_empty() => lines.iter().any(|line| line.source.enabled),
            _ => status
                .guides
                .get(&source)
                .is_some_and(|guide| !guide.configs().is_empty()),
        }
    })
}

pub(super) async fn configured_guides(
    owner: &Runtime,
    source: SourceId,
) -> Result<Vec<EpgSourceConfig>, Error> {
    match crate::commands::guide_sources(owner, source).await {
        Ok((stored, sources)) if sources.is_empty() => Ok(with_storage(
            stored
                .as_ref()
                .map(StoredGuide::configs)
                .unwrap_or_default(),
        )),
        Ok((_, sources)) => Ok(configs(&sources)),
        Err(error) => Err(error),
    }
}

/// Dispatch a prepared guide download and return its result stream.
pub(super) fn download(
    owner: &Runtime,
    source: SourceId,
    guides: Result<Vec<EpgSourceConfig>, Error>,
    cancel: CancelToken,
    visible: impl FnOnce() -> bool,
) -> (bool, UnboundedReceiver<Msg>) {
    let (tx, rx) = unbounded_channel();
    // Without a download (nothing to download, the store unreadable,
    // the workers busy) the update ends as soon as it began.
    let accepted = match guides {
        Ok(guides) if guides.is_empty() => {
            owner.emit(crate::RuntimeEvent::Notify(
                "No guide sources are configured",
            ));
            // No worker holds the sender: the wait below ends at once.
            drop(tx);
            false
        }
        Ok(_) if !owner.services().can_import() => {
            // Downloads run on blocking workers, which this host has
            // none of (the shell hides the command there); a caller
            // that asks anyway hears why nothing happens.
            owner.emit(crate::RuntimeEvent::Notify(
                "This device cannot download guides yet.",
            ));
            drop(tx);
            false
        }
        Ok(guides) => {
            let visible = visible();
            let services = owner.services();
            let accepted = services.submit_import(visible, move |worker| async move {
                download_now(&worker, source, guides, &tx, &cancel).await;
            });
            if !accepted {
                owner.emit(crate::RuntimeEvent::Notify(
                    "Too many playlists are updating. Try again shortly.",
                ));
            }
            accepted
        }
        Err(error) => {
            log::warn!("iptv data: cannot load guide sources: {error}");
            drop(tx);
            false
        }
    };
    (accepted, rx)
}

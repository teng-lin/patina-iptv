//! Favourite and recently watched channels, kept by channel key per source
//! in the store (its `favourites` and `watch_history` tables), so they
//! survive refreshes and restarts. `Ui::favorites` holds their catalog
//! channels on screen (stars, search ranking); the Favourites and Recently
//! watched lists are built from here (`lists.rs`).
//!
//! The in-memory lists are the truth while a source is shown; writes go to
//! one writer thread in order, so a toggle never waits for the store. They
//! are read when a source is shown, on the store's reader unless writes
//! of this module are still queued ([`load`]). The synthetic guide keeps
//! them in memory only.

use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};

use iptv_core::favorites::{Favorites, History, KeyIndex, RECENT_LIMIT, Recent, diff};
use iptv_core::guide::Schedule;
use iptv_providers::ContentId;
use iptv_providers::model::ContentKind;
use iptv_store_sqlite::{ChannelOrder, ChannelQuery, SourceId, Store, StoreError};

#[derive(Default)]
pub(crate) struct State {
    source: Option<SourceId>,
    combined_sources: Vec<SourceId>,
    favorites: Favorites,
    recent: Recent,
    /// The catalog on screen, by key.
    index: KeyIndex,
    /// The source has movies or shows.
    has_vod: bool,
}

/// A store write, in the order made.
pub(crate) enum Write {
    Barrier(futures_channel::oneshot::Sender<bool>),
    Shutdown(mpsc::Sender<bool>),
    MoveFavourite {
        source: SourceId,
        key: String,
        index: usize,
    },
    Favourite {
        source: SourceId,
        key: String,
        on: bool,
    },
    Watched {
        source: SourceId,
        key: String,
        at: i64,
    },
}

#[derive(Clone)]
enum PendingWrite {
    MoveFavourite {
        source: SourceId,
        key: String,
        index: usize,
    },
    Favourite {
        source: SourceId,
        key: String,
        on: bool,
    },
    Watched {
        source: SourceId,
        key: String,
        at: i64,
    },
}

/// Writes sent to the writer thread and not yet committed (or dropped with
/// their deleted playlist).
fn unsaved(owner: &super::Runtime) -> std::sync::Arc<AtomicUsize> {
    owner.favorite_unsaved.clone()
}

/// Reads saved favourites. The store's reader does not wait for a long
/// publish, but it cannot see a write still queued: while any of ours is,
/// a favourites-inbox barrier must finish before queuing the read.
async fn load<R: Send + 'static>(
    owner: &super::Runtime,
    read: impl FnOnce(&Store) -> Result<R, StoreError> + Send + 'static,
) -> Result<R, StoreError> {
    if unsaved(owner).load(Ordering::Acquire) > 0 {
        if !flush(owner).await {
            return Err(StoreError::Unavailable(
                "recent favourite edits are not saved",
            ));
        }
        owner.services().call(move |store| read(store)).await
    } else {
        owner.services().read(read).await
    }
}

/// A new app root must reload persisted favourites, including an applied restore.
pub fn reset(owner: &super::Runtime) {
    // Never reuse a previous root's epoch-bound writer when restoring a library.
    owner
        .favorite_writer
        .with(|writer| writer.borrow_mut().take());
    owner
        .favorites
        .with(|state| *state.borrow_mut() = State::default());
}

/// The source whose catalog is about to be shown (`data.rs`, before
/// `set_schedule`): its saved favourites and recent channels. The same
/// source keeps its in-memory lists (writes may still be on their way).
pub async fn prepare_source(
    owner: &std::rc::Rc<super::Runtime>,
    source: Option<SourceId>,
) -> Option<impl FnOnce()> {
    let generation = owner.generation.get();
    let id = source?;
    let loaded = load(owner, move |store| {
        let has_vod = store
            .import_status(id)
            .is_ok_and(|s| s.counts.vod + s.counts.series > 0);
        let rows = store.load_channels(
            id,
            &ChannelQuery {
                favourites_only: true,
                include_hidden: true,
                order: ChannelOrder::Effective,
                ..ChannelQuery::default()
            },
        )?;
        let recent = store.recent(Some(id), Some(ContentKind::Live), RECENT_LIMIT as u32)?;
        Ok((has_vod, rows, recent))
    })
    .await;
    if owner.generation.get() != generation {
        return None;
    }
    let (has_vod, rows, recent) = match loaded {
        Ok(loaded) => loaded,
        Err(error) => {
            log::warn!("iptv favourites: store unavailable ({error})");
            return None;
        }
    };
    let owner = owner.clone();
    Some(move || {
        owner.favorites.with(|state| {
            let mut state = state.borrow_mut();
            state.has_vod = has_vod;
            if state.source == source {
                return;
            }
            state.source = source;
            state.combined_sources.clear();
            state.favorites = Favorites::from_keys(
                rows.into_iter()
                    .map(|row| row.channel.id.as_str().to_owned()),
            );
            state.recent = Recent::from_keys(
                recent
                    .into_iter()
                    .map(|row| row.item_id.as_str().to_owned()),
            );
        })
    })
}

/// Load enabled-source favourites while preserving edits already held in memory.
pub async fn prepare_combined(owner: &std::rc::Rc<super::Runtime>) -> Option<impl FnOnce()> {
    let generation = owner.generation.get();
    use iptv_guide::combined::{qualified_key, split_key};
    let loaded = load(owner, |store| {
        let mut ids = Vec::new();
        let mut favorites = Vec::new();
        for source in store.sources()? {
            if store.source_policy(source.id)?.enabled {
                ids.push(source.id);
                let rows = store.load_channels(
                    source.id,
                    &ChannelQuery {
                        favourites_only: true,
                        include_hidden: true,
                        ..Default::default()
                    },
                )?;
                favorites.extend(
                    rows.into_iter()
                        .map(|row| (source.id, row.channel.id.as_str().to_owned())),
                );
            }
        }
        Ok((
            ids,
            favorites,
            store.recent(None, Some(ContentKind::Live), RECENT_LIMIT as u32)?,
        ))
    })
    .await;
    if owner.generation.get() != generation {
        return None;
    }
    let Ok((ids, favorites, recent)) = loaded else {
        return None;
    };
    let owner = owner.clone();
    Some(move || {
        owner.favorites.with(|state| {
            let mut state = state.borrow_mut();
            let previous = state.combined_sources.clone();
            let mut keys: Vec<_> = state
                .favorites
                .keys()
                .iter()
                .filter(|key| {
                    split_key(key).is_some_and(|(source, _)| {
                        previous.contains(&SourceId(source)) && ids.contains(&SourceId(source))
                    })
                })
                .cloned()
                .collect();
            keys.extend(
                favorites
                    .into_iter()
                    .filter(|(source, _)| !previous.contains(source))
                    .map(|(source, key)| qualified_key(source.0, &key)),
            );
            state.source = None;
            state.combined_sources = ids.clone();
            state.favorites = Favorites::from_keys(keys);
            if previous != ids {
                state.recent = Recent::from_keys(
                    recent
                        .into_iter()
                        .filter(|row| ids.contains(&row.source_id))
                        .map(|row| qualified_key(row.source_id.0, row.item_id.as_str())),
                );
            }
            state.has_vod = false;
        })
    })
}

fn write_owner(state: &State, key: &str) -> Option<(SourceId, String)> {
    if let Some(source) = state.source {
        Some((source, key.to_owned()))
    } else {
        let (s, k) = iptv_guide::combined::split_key(key)?;
        state
            .combined_sources
            .contains(&SourceId(s))
            .then(|| (SourceId(s), k.to_owned()))
    }
}

/// A new catalog is on screen (`set_schedule`): index its keys and show
/// its favourites.
pub fn catalog_changed(owner: &super::Runtime, catalog: &dyn Schedule) {
    let index = KeyIndex::build(catalog);
    owner.favorites.with(|s| s.borrow_mut().index = index);
    publish(owner);
}

pub fn publish(owner: &super::Runtime) {
    let (set, has_vod) = owner.favorites.with(|s| {
        let s = s.borrow();
        (s.favorites.set_of(&s.index), s.has_vod)
    });
    owner.emit(super::RuntimeEvent::FavoritesChanged {
        channels: set,
        has_vod,
    });
}

/// The favourites on screen as catalog channels, in their order.
pub fn favorite_channels(owner: &super::Runtime) -> Vec<usize> {
    owner.favorites.with(|s| {
        let s = s.borrow();
        s.favorites.channels(&s.index)
    })
}

/// The recently watched channels on screen, newest first.
pub fn recent_channels(owner: &super::Runtime) -> Vec<usize> {
    owner.favorites.with(|s| {
        let s = s.borrow();
        s.recent.channels(&s.index)
    })
}

/// The player's History: the recently watched channels (newest first) and
/// the row of `playing` among them.
pub fn history(owner: &super::Runtime, playing: Option<usize>) -> History {
    owner.favorites.with(|s| {
        let s = s.borrow();
        s.recent.history(&s.index, playing)
    })
}

/// The catalog channel with `key`.
pub fn channel_of(owner: &super::Runtime, key: &str) -> Option<usize> {
    owner.favorites.with(|s| s.borrow().index.get(key))
}

/// Changes favourites through their catalog channels (the editor works on
/// sets of them): what `f` adds or removes is saved by key.
pub fn edit(owner: &super::Runtime, f: impl FnOnce(&mut BTreeSet<usize>)) {
    let before = owner.favorites.with(|state| {
        let state = state.borrow();
        state.favorites.set_of(&state.index)
    });
    let mut after = before.clone();
    f(&mut after);
    let (added, removed) = diff(&before, &after);
    if added.is_empty() && removed.is_empty() {
        return;
    }
    let catalog = owner.schedule();
    for (channels, on) in [(added, true), (removed, false)] {
        for key in channels.into_iter().filter_map(|c| catalog.channel_key(c)) {
            set_key(owner, &key, on);
        }
    }
    publish(owner);
    owner.emit(super::RuntimeEvent::FavoriteOrderChanged);
}

/// Adds `channel` (catalog) to the favourites or removes it.
pub fn toggle(owner: &super::Runtime, channel: usize) {
    edit(owner, |set| {
        if !set.remove(&channel) {
            set.insert(channel);
        }
    });
}

fn set_key(owner: &super::Runtime, key: &str, on: bool) {
    let source = owner.favorites.with(|s| {
        let mut s = s.borrow_mut();
        if s.favorites.set(key, on) {
            write_owner(&s, key)
        } else {
            None
        }
    });
    if let Some((source, key)) = source {
        send(owner, Write::Favourite { source, key, on });
    }
}

/// A live channel started playing (by key).
pub fn played(owner: &super::Runtime, key: &str) {
    let source = owner.favorites.with(|s| {
        let mut s = s.borrow_mut();
        s.recent.played(key);
        write_owner(&s, key)
    });
    if let Some((source, key)) = source {
        send(
            owner,
            Write::Watched {
                source,
                key,
                at: super::imports::now_unix(),
            },
        );
    }
}

fn send(owner: &super::Runtime, write: Write) {
    let counted = !matches!(write, Write::Barrier(_) | Write::Shutdown(_));
    if counted {
        unsaved(owner).fetch_add(1, Ordering::AcqRel);
    }
    owner.favorite_writer.with(|w| {
        let mut w = w.borrow_mut();
        let tx = w.get_or_insert_with(|| {
            let (tx, rx) = mpsc::channel();
            let unsaved = unsaved(owner);
            // The writer belongs to this owner's application epoch.
            let services = owner.services();
            std::thread::spawn(move || write_all(&services, rx, unsaved));
            tx
        });
        if tx.send(write).is_err() {
            log::warn!("iptv favourites: the writer stopped");
        }
        // A write that never reached the thread is never saved; it stays
        // counted, so reads keep going behind the writer's queue.
    });
}

/// The writer thread: one store connection, writes in order.
pub async fn flush(owner: &super::Runtime) -> bool {
    let (tx, rx) = futures_channel::oneshot::channel();
    send(owner, Write::Barrier(tx));
    rx.await.unwrap_or(false)
}

/// Root teardown only: queued edits outlive their UI owner and drain first.
pub fn flush_on_shutdown(owner: &super::Runtime) -> bool {
    let writer = owner
        .favorite_writer
        .with(|writer| writer.borrow_mut().take());
    let Some(writer) = writer else { return true };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let (tx, rx) = mpsc::channel();
        if writer.send(Write::Shutdown(tx)).is_err() {
            return false;
        }
        match rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
            Ok(true) => return true,
            Ok(false) if std::time::Instant::now() < deadline => {
                // A busy database retains the head edit. Keep the engine alive
                // for bounded retries instead of treating a failed attempt as
                // a completed shutdown barrier.
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            _ => return false,
        }
    }
}

fn write_all(
    services: &super::store_service::Services,
    rx: Receiver<Write>,
    unsaved: std::sync::Arc<AtomicUsize>,
) {
    write_all_with(rx, |write| {
        let operation = write.clone();
        let result = services.blocking(move |store| match operation {
            PendingWrite::MoveFavourite { source, key, index } => store
                .move_favourite(source, &ContentId::from_stored(key), index)
                .map(|_| ()),
            PendingWrite::Favourite { source, key, on } => {
                store.set_favourite(source, &ContentId::from_stored(key), on)
            }
            PendingWrite::Watched { source, key, at } => store.record_watch(
                source,
                &ContentId::from_stored(key),
                ContentKind::Live,
                at,
                None,
            ),
        });
        // Deleting a playlist also deletes its customization/history. A queued
        // change whose owner was explicitly removed must not block other sources.
        if matches!(
            &result,
            Err(iptv_store_sqlite::StoreError::SourceNotFound(_))
        ) {
            unsaved.fetch_sub(1, Ordering::AcqRel);
            return true;
        }
        if let Err(e) = result {
            log::warn!("iptv favourites: could not save ({e})");
            return false;
        }
        unsaved.fetch_sub(1, Ordering::AcqRel);
        true
    });
}

/// A failed operation stays at the head of the queue. Neither later writes nor
/// barriers may skip it; barriers and an idle retry timer retry changes in order.
fn write_all_with(rx: Receiver<Write>, mut persist: impl FnMut(&PendingWrite) -> bool) {
    let mut pending = VecDeque::new();
    loop {
        let received = if pending.is_empty() {
            rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        } else {
            rx.recv_timeout(std::time::Duration::from_millis(250))
        };
        let barrier: Option<Box<dyn FnOnce(bool)>> = match received {
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Ok(write) => match write {
                Write::Barrier(done) => Some(Box::new(move |saved| {
                    let _ = done.send(saved);
                })),
                Write::Shutdown(done) => Some(Box::new(move |saved| {
                    let _ = done.send(saved);
                })),
                Write::MoveFavourite { source, key, index } => {
                    pending.push_back(PendingWrite::MoveFavourite { source, key, index });
                    None
                }
                Write::Favourite { source, key, on } => {
                    pending.push_back(PendingWrite::Favourite { source, key, on });
                    None
                }
                Write::Watched { source, key, at } => {
                    pending.push_back(PendingWrite::Watched { source, key, at });
                    None
                }
            },
        };
        while let Some(write) = pending.front() {
            if !persist(write) {
                break;
            }
            pending.pop_front();
        }
        if let Some(done) = barrier {
            done(pending.is_empty());
        }
    }
    if !pending.is_empty() {
        log::warn!(
            "iptv favourites: writer stopped with {} unsaved changes",
            pending.len()
        );
    }
}

/// Ordered keys of the displayed source. Reorders use the same writer queue
/// as membership changes, so rapid add/move/remove cannot race persistence.
pub fn ordered_keys(owner: &super::Runtime) -> Vec<String> {
    owner
        .favorites
        .with(|s| s.borrow().favorites.keys().to_vec())
}
pub fn reorder(owner: &super::Runtime, key: &str, index: usize) {
    let write = owner.favorites.with(|s| {
        let mut s = s.borrow_mut();
        if s.favorites.move_key(key, index) {
            move_write(&s, key, index)
        } else {
            None
        }
    });
    if let Some(write) = write {
        send(owner, write);
    }
    publish(owner);
    owner.emit(super::RuntimeEvent::FavoriteOrderChanged);
}

/// The store write for a favourite that the list in memory just moved to
/// `index`, by the owner lookup the other writes use ([`write_owner`]).
///
/// One playlist on screen: `index` is its place in that playlist's list,
/// which is what the store keeps. "All playlists": the list is every
/// playlist's favourites together, so `index` is not a place in any one
/// of them. The store keeps an order per playlist only, so the write
/// carries the favourite's place among its own playlist's favourites,
/// read from the moved list. How the playlists' favourites interleave is
/// held in memory, as for the rest of the combined list
/// ([`prepare_combined`]). (No screen reorders the combined list today:
/// the menu item is disabled there. This keeps the function right
/// should one start to.)
fn move_write(state: &State, key: &str, index: usize) -> Option<Write> {
    let (source, local) = write_owner(state, key)?;
    let index = if state.source.is_some() {
        index
    } else {
        state
            .favorites
            .keys()
            .iter()
            .take_while(|other| other.as_str() != key)
            .filter(|other| {
                iptv_guide::combined::split_key(other).is_some_and(|(id, _)| id == source.0)
            })
            .count()
    };
    Some(Write::MoveFavourite {
        source,
        key: local,
        index,
    })
}

#[cfg(test)]
mod reorder_tests {
    use super::*;
    use iptv_guide::combined::qualified_key;

    fn moved(state: &mut State, key: &str, index: usize) -> Option<(SourceId, String, usize)> {
        assert!(state.favorites.move_key(key, index));
        match move_write(state, key, index)? {
            Write::MoveFavourite { source, key, index } => Some((source, key, index)),
            _ => unreachable!(),
        }
    }

    #[test]
    fn a_load_waits_for_the_inbox_barrier_and_rejects_failed_edits() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};
        let runtime = std::rc::Rc::new(super::super::Runtime::default());
        let (writer, inbox) = mpsc::channel();
        *runtime.favorite_writer.borrow_mut() = Some(writer);
        runtime.favorite_unsaved.store(1, Ordering::Release);
        let task = {
            let runtime = runtime.clone();
            async move {
                let result = load::<()>(&runtime, |_| {
                    panic!("failed edits must not produce a stale read")
                })
                .await;
                assert!(matches!(result, Err(StoreError::Unavailable(_))));
            }
        };
        let mut task = std::pin::pin!(task);
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(task.as_mut().poll(&mut context), Poll::Pending);
        let Write::Barrier(reply) = inbox
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap()
        else {
            panic!("the read must wait behind earlier inbox edits")
        };
        reply.send(false).unwrap();
        assert_eq!(task.as_mut().poll(&mut context), Poll::Ready(()));
    }

    #[test]
    fn one_playlist_saves_the_place_in_its_own_list() {
        let mut state = State {
            source: Some(SourceId(7)),
            favorites: Favorites::from_keys(["a", "b", "c"].map(String::from)),
            ..State::default()
        };
        assert_eq!(
            moved(&mut state, "c", 0),
            Some((SourceId(7), "c".to_owned(), 0))
        );
        assert_eq!(state.favorites.keys(), ["c", "a", "b"]);
    }

    #[test]
    fn all_playlists_save_the_place_among_the_owning_playlists_favourites() {
        let key = |source, key: &str| qualified_key(source, key);
        let mut state = State {
            source: None,
            combined_sources: vec![SourceId(1), SourceId(2)],
            favorites: Favorites::from_keys([
                key(1, "a"),
                key(1, "b"),
                key(2, "x"),
                key(2, "y"),
                key(2, "z"),
            ]),
            ..State::default()
        };
        // Last of the list to its front: first of playlist 2's own.
        assert_eq!(
            moved(&mut state, &key(2, "z"), 0),
            Some((SourceId(2), "z".to_owned(), 0))
        );
        // To the fourth place of the combined list: z, a and b lie before
        // it, of which only z is playlist 2's.
        assert_eq!(
            moved(&mut state, &key(2, "y"), 3),
            Some((SourceId(2), "y".to_owned(), 1))
        );
        assert_eq!(
            state.favorites.keys(),
            [
                key(2, "z"),
                key(1, "a"),
                key(1, "b"),
                key(2, "y"),
                key(2, "x")
            ]
        );
        // Playlist 1's own order is untouched by those: b to the end is
        // its second place.
        assert_eq!(
            moved(&mut state, &key(1, "b"), 4),
            Some((SourceId(1), "b".to_owned(), 1))
        );
    }

    #[test]
    fn a_favourite_of_no_shown_playlist_is_not_written() {
        let stray = qualified_key(9, "gone");
        let mut state = State {
            source: None,
            combined_sources: vec![SourceId(1)],
            favorites: Favorites::from_keys([qualified_key(1, "a"), stray.clone()]),
            ..State::default()
        };
        assert_eq!(moved(&mut state, &stray, 0), None);
        // A key that is not a combined one has no owner either.
        state.favorites = Favorites::from_keys(["bare".to_owned()]);
        assert_eq!(moved(&mut state, "bare", 0), None);
    }
}

#[cfg(test)]
mod writer_tests {
    use super::*;

    #[test]
    fn failed_changes_retry_in_order_before_a_barrier_can_succeed() {
        let (tx, rx) = mpsc::channel();
        tx.send(Write::Favourite {
            source: SourceId(1),
            key: "generated-a".into(),
            on: true,
        })
        .unwrap();
        tx.send(Write::MoveFavourite {
            source: SourceId(1),
            key: "generated-a".into(),
            index: 0,
        })
        .unwrap();
        let (first, mut first_result) = futures_channel::oneshot::channel();
        tx.send(Write::Barrier(first)).unwrap();
        let (retry, mut retry_result) = futures_channel::oneshot::channel();
        tx.send(Write::Barrier(retry)).unwrap();
        tx.send(Write::Watched {
            source: SourceId(2),
            key: "generated-b".into(),
            at: 123,
        })
        .unwrap();
        let (last, mut last_result) = futures_channel::oneshot::channel();
        tx.send(Write::Barrier(last)).unwrap();
        drop(tx);
        let mut attempts = 0;
        let mut persisted = Vec::new();
        write_all_with(rx, |write| {
            attempts += 1;
            if attempts <= 3 {
                return false;
            }
            persisted.push(match write {
                PendingWrite::Favourite { on, .. } => {
                    assert!(*on);
                    "favorite"
                }
                PendingWrite::MoveFavourite { index, .. } => {
                    assert_eq!(*index, 0);
                    "order"
                }
                PendingWrite::Watched { source, at, .. } => {
                    assert_eq!((*source, *at), (SourceId(2), 123));
                    "history"
                }
            });
            true
        });
        assert_eq!(first_result.try_recv().unwrap(), Some(false));
        assert_eq!(retry_result.try_recv().unwrap(), Some(true));
        assert_eq!(last_result.try_recv().unwrap(), Some(true));
        assert_eq!(persisted, ["favorite", "order", "history"]);
    }

    #[test]
    fn repeated_flushes_cannot_clear_a_persistent_failure() {
        let (tx, rx) = mpsc::channel();
        tx.send(Write::Favourite {
            source: SourceId(1),
            key: "generated".into(),
            on: false,
        })
        .unwrap();
        let mut answers = Vec::new();
        for _ in 0..3 {
            let (done, result) = futures_channel::oneshot::channel();
            tx.send(Write::Barrier(done)).unwrap();
            answers.push(result);
        }
        drop(tx);
        let mut attempts = 0;
        write_all_with(rx, |_| {
            attempts += 1;
            false
        });
        assert_eq!(attempts, 4);
        for mut result in answers {
            assert_eq!(result.try_recv().unwrap(), Some(false));
        }
    }
}

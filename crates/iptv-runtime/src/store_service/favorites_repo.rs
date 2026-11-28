//! Typed favorites and watch-history persistence.

use super::{Services, Store, StoreError};
use iptv_core::favorites::RECENT_LIMIT;
use iptv_library::SourceId;
use iptv_providers::{ContentId, model::ContentKind};
use std::future::Future;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

pub(crate) const UNSAVED_EDITS: StoreError =
    StoreError::Unavailable("recent favorite edits are not saved");

/// Keeps a write counted in the unsaved-edit counter until it ran or was dropped.
pub(crate) struct Counted(Arc<AtomicUsize>);

impl Counted {
    pub(crate) fn new(unsaved: Arc<AtomicUsize>) -> Self {
        unsaved.fetch_add(1, Ordering::AcqRel);
        Self(unsaved)
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Clone)]
pub(crate) enum FavoriteWrite {
    ClearHistory,
    MoveFavorite {
        source: SourceId,
        key: String,
        index: usize,
    },
    Favorite {
        source: SourceId,
        key: String,
        on: bool,
    },
    Watched {
        source: SourceId,
        key: String,
        at: i64,
    },
    WatchTime {
        source: SourceId,
        key: String,
        elapsed_ms: u64,
    },
}

pub(crate) struct SourceData {
    pub has_vod: bool,
    pub any_vod: Option<bool>,
    pub favorites: Vec<String>,
    pub recent: Vec<String>,
}

pub(crate) struct CombinedData {
    pub sources: Vec<SourceId>,
    pub favorites: Vec<(SourceId, Vec<String>)>,
    pub recent: Vec<(SourceId, String)>,
    pub has_vod: Option<bool>,
}

pub(crate) struct FavoritesRepo {
    pub(super) services: Services,
}

fn source_has_vod(store: &Store, source: SourceId) -> Result<bool, StoreError> {
    let counts = store.import_status(source)?.counts;
    Ok(counts.vod > 0 || counts.series > 0)
}

fn any_enabled_vod(store: &Store) -> Result<bool, StoreError> {
    for source in store.sources()? {
        if store.source_policy(source.id)?.enabled && source_has_vod(store, source.id)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn persist(store: &mut Store, write: FavoriteWrite) -> Result<(), StoreError> {
    match write {
        FavoriteWrite::ClearHistory => store.clear_history(None).map(|_| ()),
        FavoriteWrite::MoveFavorite { source, key, index } => store
            .move_favorite(source, &ContentId::from_stored(key), index)
            .map(|_| ()),
        FavoriteWrite::Favorite { source, key, on } => {
            store.set_favorite(source, &ContentId::from_stored(key), on)
        }
        FavoriteWrite::WatchTime {
            source,
            key,
            elapsed_ms,
        } => store.add_channel_watch_time(source, &ContentId::from_stored(key), elapsed_ms),
        FavoriteWrite::Watched { source, key, at } => store.record_watch(
            source,
            &ContentId::from_stored(key),
            ContentKind::Live,
            at,
            None,
        ),
    }
}

impl FavoritesRepo {
    async fn read<R: Send + 'static>(
        &self,
        after_writes: bool,
        load: impl FnOnce(&Store) -> Result<R, StoreError> + Send + 'static,
    ) -> Result<R, StoreError> {
        if after_writes {
            self.services.call(move |store| load(store)).await
        } else {
            self.services.read(load).await
        }
    }

    pub(crate) async fn source_data(
        self,
        source: SourceId,
        after_writes: bool,
    ) -> Result<SourceData, StoreError> {
        self.read(after_writes, move |store| {
            let has_vod = source_has_vod(store, source).unwrap_or(false);
            let any_vod = if has_vod {
                Some(true)
            } else {
                any_enabled_vod(store).ok()
            };
            Ok(SourceData {
                has_vod,
                any_vod,
                favorites: store.favorite_keys(source)?,
                recent: store
                    .recent(Some(source), Some(ContentKind::Live), RECENT_LIMIT as u32)?
                    .into_iter()
                    .map(|row| row.item_id.as_str().to_owned())
                    .collect(),
            })
        })
        .await
    }

    pub(crate) async fn combined_data(
        self,
        after_writes: bool,
    ) -> Result<CombinedData, StoreError> {
        self.read(after_writes, |store| {
            let mut sources = Vec::new();
            let mut favorites = Vec::new();
            let mut has_vod = false;
            let mut availability_known = true;
            for source in store.sources()? {
                if store.source_policy(source.id)?.enabled {
                    sources.push(source.id);
                    match source_has_vod(store, source.id) {
                        Ok(available) => has_vod |= available,
                        Err(_) => availability_known = false,
                    }
                    favorites.push((source.id, store.favorite_keys(source.id)?));
                }
            }
            let recent = store
                .recent(None, Some(ContentKind::Live), RECENT_LIMIT as u32)?
                .into_iter()
                .map(|row| (row.source_id, row.item_id.as_str().to_owned()))
                .collect();
            Ok(CombinedData {
                sources,
                favorites,
                recent,
                has_vod: (has_vod || availability_known).then_some(has_vod),
            })
        })
        .await
    }

    pub(crate) fn any_enabled_vod(self) -> impl Future<Output = Result<bool, StoreError>> + use<> {
        self.services.read(any_enabled_vod)
    }

    pub(crate) fn enqueue_write(
        self,
        write: FavoriteWrite,
        on_result: impl FnOnce(&Result<(), StoreError>) + Send + 'static,
    ) {
        drop(self.services.call(move |store| {
            let result = persist(store, write);
            on_result(&result);
            result
        }));
    }

    pub(crate) fn persist_blocking(&self, write: FavoriteWrite) -> Result<(), StoreError> {
        self.services.blocking(move |store| persist(store, write))
    }

    pub(crate) async fn save_order_cooperatively(
        self,
        source: SourceId,
        original: Vec<String>,
        order: Vec<String>,
        failed: Arc<AtomicBool>,
        counted: Counted,
    ) -> Result<(), StoreError> {
        self.services
            .call(move |store| {
                let _counted = counted;
                if failed.swap(false, Ordering::AcqRel) {
                    return Err(UNSAVED_EDITS);
                }
                store.save_favorite_order(source, &original, &order)
            })
            .await
    }

    pub(crate) fn save_order_blocking(
        &self,
        source: SourceId,
        original: Vec<String>,
        order: Vec<String>,
    ) -> Result<(), StoreError> {
        self.services
            .blocking(move |store| store.save_favorite_order(source, &original, &order))
    }
}

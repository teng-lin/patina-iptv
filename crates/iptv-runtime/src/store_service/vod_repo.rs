//! VOD catalog snapshots and cancellable shelf queries.

use super::{Services, StoreError};
use iptv_core::vod::{Card, Category, DiscoveryPage, DiscoveryQuery, Key, Query, Shelf, Source};
use iptv_import::CancelToken;

type CatalogPages = (Vec<Card>, bool, Vec<Source>, Vec<Category>);

pub(crate) struct VodRepo {
    pub(super) services: Services,
}

impl VodRepo {
    pub(crate) async fn catalog_pages(
        self,
        query: Query,
        offset: u32,
        at_least: u32,
    ) -> Result<CatalogPages, StoreError> {
        const PAGE: u32 = 80;
        self.services
            .read(move |s| {
                s.vod_read_snapshot(|s| {
                    let mut cards = Vec::new();
                    let mut more = true;
                    let mut read = 0;
                    // `read` counts store rows consumed, as Load more does.
                    while more && read < at_least {
                        let (page, next) = s.vod_page(&query, offset.saturating_add(read), PAGE)?;
                        read = read.saturating_add(u32::try_from(page.len()).unwrap_or(u32::MAX));
                        more = next;
                        cards.extend(page);
                    }
                    Ok((cards, more, s.vod_sources()?, s.vod_categories(&query)?))
                })
            })
            .await
    }

    pub(crate) async fn discovery(
        self,
        query: DiscoveryQuery,
        cancel: CancelToken,
    ) -> Result<DiscoveryPage, StoreError> {
        self.services
            .analyze(move |store| store.vod_discovery_cancelable(query, &cancel))
            .await
    }

    pub(crate) async fn discovery_around(
        self,
        shelf: Shelf,
        key: Key,
        cancel: CancelToken,
    ) -> Result<DiscoveryPage, StoreError> {
        self.services
            .analyze(move |store| store.vod_discovery_around_cancelable(shelf, &key, &cancel))
            .await
    }
}

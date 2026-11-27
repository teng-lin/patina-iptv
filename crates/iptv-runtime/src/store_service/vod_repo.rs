//! Typed VOD catalog and progress repository.

use super::{Services, StoreError};
use iptv_core::vod::{Card, Category, DiscoveryPage, DiscoveryQuery, Key, Progress, Query, Source};

pub(crate) struct VodRepo {
    pub(super) services: Services,
}

impl VodRepo {
    pub(crate) async fn sources(&self) -> Result<Vec<Source>, StoreError> {
        self.services.read(|store| store.vod_sources()).await
    }

    pub(crate) async fn categories(self, query: Query) -> Result<Vec<Category>, StoreError> {
        self.services
            .read(move |store| store.vod_categories(&query))
            .await
    }

    pub(crate) async fn page(
        self,
        query: Query,
        offset: u32,
        limit: u32,
    ) -> Result<(Vec<Card>, bool), StoreError> {
        self.services
            .read(move |store| store.vod_page(&query, offset, limit))
            .await
    }

    pub(crate) async fn discovery(
        self,
        query: DiscoveryQuery,
    ) -> Result<DiscoveryPage, StoreError> {
        self.services
            .read(move |store| store.vod_discovery(query))
            .await
    }

    pub(crate) async fn card(self, key: Key) -> Result<Option<Card>, StoreError> {
        self.services.read(move |store| store.vod_card(&key)).await
    }

    pub(crate) async fn progress(self, key: Key) -> Result<Progress, StoreError> {
        self.services
            .read(move |store| store.vod_progress(&key))
            .await
    }

    pub(crate) async fn resume_episode(self, key: Key) -> Result<Option<Key>, StoreError> {
        self.services
            .read(move |store| store.vod_resume_episode(&key))
            .await
    }
}

//! Typed application metadata and history repository.

use super::{Connection, Service, Services, Store, StoreError};
use iptv_search::SearchHistory;
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) struct SystemRepo {
    pub(super) services: Services,
}

impl SystemRepo {
    pub(crate) async fn load_search_history(
        self,
        legacy: PathBuf,
    ) -> Result<Vec<String>, StoreError> {
        self.services
            .call(move |store| {
                if let Ok(bytes) = std::fs::read(&legacy) {
                    let list: Vec<String> = serde_json::from_slice(&bytes).unwrap_or_default();
                    if !list.is_empty() && store.search_history()?.is_empty() {
                        let list = SearchHistory::from_list(list, SearchHistory::DEFAULT_CAPACITY)
                            .to_list();
                        store.set_search_history(&list)?;
                        log::info!(
                            "iptv search: moved {} recent searches into the store",
                            list.len()
                        );
                    }
                    if let Err(error) = std::fs::remove_file(&legacy)
                        && error.kind() != std::io::ErrorKind::NotFound
                    {
                        log::warn!(
                            "iptv search: could not delete the old recent searches file ({})",
                            error.kind()
                        );
                    }
                }
                store.search_history()
            })
            .await
    }

    pub(crate) async fn search_history(&self) -> Result<Vec<String>, StoreError> {
        self.services.read(|store| store.search_history()).await
    }

    pub(crate) async fn set_search_history(self, history: Vec<String>) -> Result<(), StoreError> {
        self.services
            .call(move |store| store.set_search_history(&history))
            .await
    }

    pub(crate) async fn clear_search_history(self, legacy: PathBuf) -> Result<(), StoreError> {
        self.services
            .call(move |store| {
                store.set_search_history(&[])?;
                match std::fs::remove_file(legacy) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(_) => Err(StoreError::Unavailable(
                        "legacy search history could not be removed",
                    )),
                }
            })
            .await
    }
}

impl Service {
    /// A host that keeps the store persists what the last writer job
    /// committed (natively the file is the persistence: nothing to do).
    pub(crate) fn persist(&self, store: &Store) {
        let Some(host) = &self.ports.store_host else {
            return;
        };
        let me = self.me.get().cloned().unwrap_or_default();
        host.committed(
            store,
            Arc::new(move || {
                if let Some(service) = me.upgrade() {
                    service.queue_persist();
                }
            }),
        );
    }

    /// Persists again on the writer, after what is queued there.
    fn queue_persist(self: &Arc<Self>) {
        let owner = self.clone();
        let _ = self
            .writer
            .queue(Box::new(move |connection: &mut Connection| {
                if let Some(store) = &connection.store {
                    owner.persist(store);
                }
            }));
    }
}

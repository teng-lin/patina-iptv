//! Named shell operations; mutable cells never cross the runtime boundary.
use crate::Runtime;
use iptv_core::wizard::ImportId;
use iptv_import::CancelToken;
use iptv_library::SourceId;
use std::{future::Future, rc::Rc};

impl Runtime {
    pub fn browser_generation(&self) -> u64 {
        self.generation.get()
    }
    pub fn account_limit(&self) -> Option<u32> {
        self.account_streams.get()
    }
    pub fn update_account_limit(&self, limit: Option<u32>) {
        self.account_streams.set(limit);
    }
    pub fn set_foreground(&self, foreground: bool) {
        self.foreground.set(foreground);
    }
    pub fn cancel_link_resolution(&self, surface: usize) {
        let running = self
            .resolving
            .borrow_mut()
            .get_mut(surface)
            .and_then(Option::take);
        if let Some((_, cancel)) = running {
            cancel.cancel();
        }
    }
    pub fn begin_link_resolution(&self, surface: usize, session: u64) -> CancelToken {
        self.cancel_link_resolution(surface);
        let cancel = CancelToken::new();
        if let Some(slot) = self.resolving.borrow_mut().get_mut(surface) {
            *slot = Some((session, cancel.clone()));
        }
        cancel
    }
    pub fn finish_link_resolution(&self, surface: usize, session: u64) {
        if let Some(slot) = self.resolving.borrow_mut().get_mut(surface)
            && slot.as_ref().is_some_and(|(at, _)| *at == session)
        {
            *slot = None;
        }
    }
    pub fn remember_wizard_import(&self, id: ImportId, cancel: CancelToken) {
        *self.wizard_import.borrow_mut() = Some((id, cancel));
    }
    pub fn cancel_wizard_import(&self, id: ImportId) {
        let mut running = self.wizard_import.borrow_mut();
        if running.as_ref().is_some_and(|(at, _)| *at == id)
            && let Some((_, token)) = running.take()
        {
            log::info!("iptv wizard: import {id:?} cancelled");
            token.cancel();
        }
    }
    pub fn finish_wizard_import(&self) {
        self.wizard_import.borrow_mut().take();
    }
    pub fn search_history(&self) -> Vec<String> {
        self.saved_search.borrow().clone().unwrap_or_default()
    }
    pub fn clear_loaded_search_history(&self) {
        *self.saved_search.borrow_mut() = None;
    }
    pub fn install_search_history(&self, history: Vec<String>) {
        *self.saved_search.borrow_mut() = Some(history);
    }
    pub fn search_history_matches(&self, history: &[String]) -> bool {
        self.saved_search.borrow().as_deref() == Some(history)
    }
    pub fn begin_search(&self) -> u64 {
        let next = self.pending_search.get() + 1;
        self.pending_search.set(next);
        next
    }
    pub fn search_is_current(&self, ticket: u64) -> bool {
        self.pending_search.get() == ticket
    }

    /// Starts a backup operation unless one is running (the user is told).
    pub fn begin_backup(self: &Rc<Self>) -> Option<crate::backup::Busy> {
        crate::backup::Busy::begin(self.clone())
    }

    /// A restore is scheduled for the next launch.
    pub fn restore_is_pending(&self) -> bool {
        crate::backup::is_pending(self.data_dir())
    }

    /// Holds this owner's storage for the life of the process. Call before
    /// settings, restore activation or any storage worker starts.
    pub fn lock_storage(&self) -> Result<(), crate::backup::error::Error> {
        crate::backup::lock(self.data_dir())
    }

    /// The activation of a scheduled restore (or the recovery of an
    /// interrupted one), to run off the UI thread before anything under
    /// this owner's storage is opened.
    pub fn restore_activation(
        &self,
    ) -> impl FnOnce() -> Result<(), crate::backup::StartupError> + Send + 'static + use<> {
        let ports = self.services().ports().clone();
        move || crate::backup::startup(&ports)
    }

    /// Reuses connections across Stalker zaps; a settings change replaces
    /// the client.
    pub fn link_client(
        &self,
        config: iptv_net::NetConfig,
    ) -> impl Future<Output = Result<iptv_net::NetClient, iptv_net::FetchError>> + Send + use<>
    {
        crate::playback::link_client(self, config)
    }

    /// Runs `future` on this owner's network runtime (link resolves,
    /// artwork); await the handle for its output.
    pub fn spawn_network<F>(&self, future: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.services().spawn(future)
    }

    /// The effective settings (defaults until [`Runtime::load_settings`]).
    pub fn settings(&self) -> std::sync::Arc<iptv_settings::Settings> {
        self.services().settings()
    }

    pub fn load_settings(&self) {
        crate::settings::load(self);
    }

    pub fn set_setting(
        self: &Rc<Self>,
        key: &str,
        value: iptv_settings::Value,
    ) -> Result<(), crate::settings::SaveError> {
        crate::settings::set(self, key, value)
    }

    /// Remembers `key` as the last channel of `source` (a zap never waits
    /// for the disk).
    pub fn save_last_channel(&self, source: Option<SourceId>, key: &str) {
        crate::playback::save_last_channel(&self.services(), source, key);
    }

    /// The last channel of `source`, a write still queued included.
    pub fn load_last_channel(&self, source: Option<SourceId>) -> Option<String> {
        crate::playback::load_last_channel(&self.services(), source)
    }

    /// Playlist `source` was deleted: its last channel goes too.
    pub fn forget_last_channel(&self, source: SourceId) {
        crate::playback::forget_source(&self.services(), source);
    }

    /// Remembers `key` as the last channel of the combined browser.
    pub fn save_combined_channel(&self, key: &str) {
        crate::playback::save_combined_channel(&self.services(), key);
    }

    /// The last channel of the combined browser, a queued write included.
    pub fn load_combined_channel(&self) -> Option<String> {
        crate::playback::load_combined_channel(&self.services())
    }

    /// Keeps a picked playlist file in this owner's files (a picked
    /// document is only readable now; a refresh reads the copy). Returns its
    /// path.
    pub fn keep_picked_file(
        &self,
        name: &str,
        bytes: &[u8],
    ) -> impl Future<Output = std::io::Result<String>> + use<> {
        let services = self.services();
        let (name, bytes) = (name.to_owned(), bytes.to_vec());
        async move { crate::imports::keep_picked_file(&services, &name, &bytes).await }
    }

    pub fn reset_favorites(&self) {
        crate::favorites::reset(self);
    }

    pub fn favorites_catalog_changed(&self, catalog: &dyn iptv_core::guide::Schedule) {
        crate::favorites::catalog_changed(self, catalog);
    }

    pub fn favorite_channels(&self) -> Vec<usize> {
        crate::favorites::favorite_channels(self)
    }

    pub fn recent_channels(&self) -> Vec<usize> {
        crate::favorites::recent_channels(self)
    }

    pub fn favorites_history(&self, playing: Option<usize>) -> iptv_core::favorites::History {
        crate::favorites::history(self, playing)
    }

    pub fn favorites_channel_of(&self, key: &str) -> Option<usize> {
        crate::favorites::channel_of(self, key)
    }

    pub fn edit_favorites(&self, f: impl FnOnce(&mut std::collections::BTreeSet<usize>)) {
        crate::favorites::edit(self, f);
    }

    pub fn toggle_favorite(&self, channel: usize) {
        crate::favorites::toggle(self, channel);
    }

    pub fn record_played(&self, key: &str) {
        crate::favorites::played(self, key);
    }

    pub fn ordered_favorite_keys(&self) -> Vec<String> {
        crate::favorites::ordered_keys(self)
    }

    pub fn reorder_favorite(&self, key: &str, index: usize) {
        crate::favorites::reorder(self, key, index);
    }

    /// Resolves once the favourite and history edits made so far are saved;
    /// false when they could not be.
    pub fn flush_favorites(self: &Rc<Self>) -> impl Future<Output = bool> + use<> {
        let owner = self.clone();
        async move { crate::favorites::flush(&owner).await }
    }

    pub fn flush_favorites_on_shutdown(&self) -> bool {
        crate::favorites::flush_on_shutdown(self)
    }

    pub fn shown_source(&self) -> Option<SourceId> {
        crate::stored::shown(self)
    }

    pub fn shown_sources(&self) -> Vec<SourceId> {
        crate::stored::shown_sources(self)
    }

    pub fn guide_status(&self) -> crate::stored::GuideStatus {
        crate::stored::guide_status(self)
    }

    pub fn is_updating(&self, source: SourceId) -> bool {
        crate::stored::updating(self, source)
    }

    pub fn source_lines(&self, source: SourceId) -> Vec<iptv_library::SourceLine> {
        crate::stored::source_lines(self, source)
    }

    pub fn cached_guide_state(&self, source: SourceId) -> Option<iptv_library::GuideSnapshot> {
        crate::stored::cached_guide_state(self, source)
    }

    pub fn playlists(&self) -> Result<Vec<iptv_library::Playlist>, crate::Error> {
        crate::library::playlists(self)
    }

    /// Stored playlist `id` with its details (secrets included: for the
    /// details form only, never shown or logged).
    pub fn source_details(&self, id: SourceId) -> Option<iptv_providers::model::Source> {
        crate::library::source_details(self, id)
    }

    pub fn is_browsing_all(&self) -> bool {
        self.all.get()
    }
}

#[cfg(feature = "test-support")]
impl Runtime {
    /// Occupies the store writer for `duration` after the work queued
    /// before it.
    pub fn occupy_writer(&self, duration: std::time::Duration) {
        self.services().occupy_writer(duration);
    }

    /// A stored source's catalogue size as an import worker reads it (a
    /// blocking read on the reader lane), to run on another thread.
    pub fn worker_catalog_size(
        &self,
        source: SourceId,
    ) -> impl FnOnce() -> Result<(usize, usize), crate::Error> + Send + 'static + use<> {
        let services = self.services();
        move || {
            let (groups, channels) = crate::stored::catalog(&services, source)?;
            Ok((groups.len(), channels.len()))
        }
    }
}

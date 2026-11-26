//! Named shell operations; mutable cells never cross the runtime boundary.
use crate::Runtime;
use crate::commands::external_playback_check;
use iptv_core::add_playlist::ImportId;
use iptv_core::parental::Target;
use iptv_guide::playback::ResolutionAuthority;
use iptv_import::CancelToken;
use iptv_library::SourceId;
use std::{future::Future, rc::Rc};

/// Parental policy verified against committed storage for external playback.
pub struct ExternalPlaybackAuthorization {
    access: crate::parental::Snapshot,
    target: Target,
    grants: Vec<Target>,
}

impl ExternalPlaybackAuthorization {
    pub(crate) fn new(
        access: crate::parental::Snapshot,
        target: Target,
        grants: Vec<Target>,
    ) -> Self {
        Self {
            access,
            target,
            grants,
        }
    }

    /// Check immediately before granting the relay admission, without yielding.
    pub fn is_current(&self) -> bool {
        self.access.playback_current(&self.target, &self.grants)
    }
}

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
        self.set_foreground_for_external(foreground, None);
    }
    /// The native player chooser backgrounds the guide during a user-approved handoff.
    pub fn set_foreground_for_external(
        &self,
        foreground: bool,
        external: Option<&iptv_core::parental::Target>,
    ) {
        self.foreground.set(foreground);
        if !foreground {
            if let Some(target) = external {
                self.parental_content_transferred(target);
            } else {
                self.parental_backgrounded();
            }
        }
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
            log::info!("iptv wizard: import {id:?} canceled");
            token.cancel();
        }
    }
    pub fn finish_wizard_import(&self) {
        self.wizard_import.borrow_mut().take();
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
        if !self.services().ports().file_storage {
            // One Worker owns a browser host's storage; there is no file to lock.
            return Ok(());
        }
        crate::backup::lock(self.data_dir())
    }

    /// The activation of a scheduled restore (or the recovery of an
    /// interrupted one), to run off the UI thread before anything under
    /// this owner's storage is opened.
    pub fn restore_activation(
        &self,
    ) -> impl FnOnce() -> Result<(), crate::backup::StartupError> + Send + 'static + use<> {
        let ports = self.services().ports().clone();
        move || {
            if !ports.file_storage {
                // No files, so no restore can be pending (backups need them).
                return Ok(());
            }
            if crate::backup::is_pending(&ports.directory)
                && !crate::recordings::host::stop_background_host(&ports.directory)
            {
                return Err(crate::backup::StartupError {
                    error: iptv_core::message::key(
                        "Recording host did not close; restart Patina before restoring",
                    )
                    .into(),
                    fatal: true,
                });
            }
            crate::recordings::host::activate_storage(&ports)
        }
    }

    /// Reuses connections across Stalker zaps; a settings change replaces
    /// the client.
    pub fn link_client(
        &self,
        config: crate::network::NetworkConfig,
    ) -> impl Future<Output = Result<crate::network::Client, iptv_import::FetchError>>
    + crate::MaybeSend
    + use<> {
        crate::playback::link_client(self, config)
    }

    /// Tells the user, once per owner, that settings last for the session:
    /// this host has neither a file writer nor a file host to save them.
    pub(crate) fn note_session_only(&self) {
        if !self.session_only_noted.replace(true) {
            self.emit(crate::RuntimeEvent::Notify(
                "Changes are not saved on this device: they last until you show another playlist or close Patina.",
            ));
        }
    }

    /// The host has blocking workers: staged probes and backups need them,
    /// and natively imports, guide downloads and the file and favorites
    /// writers run on them. A cooperative browser host has none: its shell
    /// offers no probes or backups, and the others take their cooperative
    /// forms there.
    pub fn has_workers(&self) -> bool {
        self.services().has_workers()
    }

    /// This owner's network port (artwork makes its own client with its own
    /// limits, off the UI thread).
    pub fn network(&self) -> std::sync::Arc<dyn crate::network::Network> {
        self.services().ports().network.clone()
    }

    /// Runs `future` on this owner's network runtime (link resolves,
    /// artwork); await the handle for its output.
    pub fn spawn_network<F>(&self, future: F) -> crate::execution::Join<F::Output>
    where
        F: Future + crate::MaybeSend + 'static,
        F::Output: crate::MaybeSend + 'static,
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

    /// Checks the current settings authorization and storage state before
    /// starting an operation that will update this setting.
    pub fn check_setting_write(&self, key: &str) -> Result<(), crate::settings::SaveError> {
        crate::settings::check_setting_write(self, key)
    }

    pub fn set_setting(
        self: &Rc<Self>,
        key: &str,
        value: iptv_settings::Value,
    ) -> Result<(), crate::settings::SaveError> {
        crate::settings::set(self, key, value)
    }

    /// Stores `value` for `key` as playlist `source`'s own setting.
    pub fn set_playlist_setting(
        self: &Rc<Self>,
        source: SourceId,
        key: &str,
        value: iptv_settings::Value,
    ) -> Result<(), crate::settings::SaveError> {
        let at = iptv_settings::Target::playlist(source.0.to_string());
        crate::settings::set_in(self, key, &at, value)
    }

    /// The streams at once `source`'s provider reported for its account
    /// (an Xtream account's `max_connections`), if an import of this
    /// session read it. A playlist's own `playlists.max_connections`
    /// overrides it; the playlists screen shows both.
    pub fn reported_streams(&self, source: SourceId) -> Option<u32> {
        if self.shown.get() == Some(source)
            && let Some(limit) = self.account_streams.get()
        {
            return Some(limit);
        }
        if let Some(Some(limit)) = self.limits.borrow().get(&source) {
            return Some(*limit);
        }
        if let Some(single) = self.single.borrow().as_ref()
            && single.source == source
            && single.limit.is_some()
        {
            return single.limit;
        }
        self.recorded_provider_limit(source)
    }

    /// Sets playlist `source`'s own streams-at-once ceiling, or (`None`)
    /// removes it so the allowance its provider reports applies.
    pub fn set_connection_limit(
        self: &Rc<Self>,
        source: SourceId,
        limit: Option<u32>,
    ) -> Result<(), crate::settings::SaveError> {
        crate::settings::set_own(
            self,
            iptv_settings::keys::PLAYLISTS_MAX_CONNECTIONS.as_str(),
            &iptv_settings::Target::playlist(source.0.to_string()),
            limit.map(|n| iptv_settings::Value::Int(i64::from(n))),
        )
    }

    /// Revalidates the group choice and the exact authority used for a handoff.
    pub async fn external_playback_allowed(
        &self,
        authority: ResolutionAuthority,
        group: String,
    ) -> Result<bool, crate::Error> {
        let access = self.parental_snapshot();
        Ok(self
            .services()
            .read(move |store| {
                Ok(external_playback_check(store, &authority, &group, access)?.is_some())
            })
            .await?)
    }

    /// Orders final admission behind queued parental changes. The returned
    /// policy revision also detects changes queued after this check.
    pub async fn authorize_external_playback(
        &self,
        authority: ResolutionAuthority,
        group: String,
    ) -> Result<Option<ExternalPlaybackAuthorization>, crate::Error> {
        let access = self.parental_snapshot();
        Ok(self
            .services()
            .call(move |store| external_playback_check(store, &authority, &group, access))
            .await?)
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

    pub fn favorite_channels(&self) -> Vec<usize> {
        crate::favorites::favorite_channels(self)
    }

    pub fn recent_channels(&self) -> Vec<usize> {
        crate::favorites::recent_channels(self)
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

    /// Clears persisted viewing history and invalidates the recent-channel list.
    pub fn clear_watch_history(&self) -> impl Future<Output = bool> + use<> {
        crate::favorites::clear_history(self)
    }

    pub fn record_played(&self, key: &str) {
        crate::favorites::played(self, key);
    }

    /// Queues sampled live playback time against the original playlist and channel identity.
    pub fn add_channel_watch_time(&self, source: SourceId, key: String, elapsed_ms: u64) {
        crate::favorites::watch_time(self, source, key, elapsed_ms);
    }

    /// Applies new statistics on entering the guide or selecting a different list.
    pub fn refresh_watch_order(self: &Rc<Self>) {
        let dirty = std::mem::take(&mut *self.watch_sort_dirty.borrow_mut());
        if dirty.is_empty() {
            return;
        }
        let owner = self.clone();
        self.spawn(async move {
            if !crate::favorites::flush(&owner).await {
                owner.watch_sort_dirty.borrow_mut().extend(dirty);
                return;
            }
            let sources = owner
                .services()
                .call({
                    let dirty = dirty.clone();
                    move |store| {
                        let mut affected = Vec::new();
                        for source in dirty {
                            if store.has_watch_sort(source)? {
                                affected.push(source);
                            }
                        }
                        Ok(affected)
                    }
                })
                .await;
            match sources {
                Ok(sources) => {
                    for source in sources {
                        crate::data::watch_order_changed(&owner, source);
                    }
                }
                Err(_) => owner.watch_sort_dirty.borrow_mut().extend(dirty),
            }
        });
    }

    pub fn ordered_favorite_keys(&self) -> Vec<String> {
        crate::favorites::ordered_keys(self)
    }

    pub fn reorder_favorite(&self, key: &str, index: usize) {
        crate::favorites::reorder(self, key, index);
    }

    pub async fn save_favorite_order(
        &self,
        source: SourceId,
        original: Vec<String>,
        order: Vec<String>,
    ) -> Result<(), crate::Error> {
        Ok(crate::favorites::save_order(self, source, original, order).await?)
    }

    /// Resolves once the favorite and history edits made so far are saved;
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
    /// Captures completion of the current synthetic import before it is revoked.
    pub fn pending_import_completion(
        &self,
        source: SourceId,
    ) -> Option<impl Future<Output = ()> + use<>> {
        crate::library::pending_import(self, source).map(|job| async move {
            job.until_finished().await;
        })
    }

    /// Occupies the store writer for `duration` after the work queued
    /// before it.
    pub fn occupy_writer(&self, duration: std::time::Duration) {
        self.services().occupy_writer(duration);
    }

    /// Holds the actual analysis lane behind a test-owned release barrier.
    /// Entry acknowledgement makes contention tests independent of scheduling.
    /// Dropping the release sender, or the watchdog, always releases the lane.
    pub fn hold_analysis(&self) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        self.services().hold_analysis()
    }

    /// Pauses the next inspection after SQLite has actually executed scan work.
    /// The bounded test barrier never changes a production build's behavior.
    pub fn hold_inspection_scan(
        &self,
    ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        let (entered, observed) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel();
        *self.inspection_scan.borrow_mut() = Some(Box::new(move || {
            let _ = entered.send(());
            let _ = resume.recv_timeout(std::time::Duration::from_secs(10));
        }));
        (observed, release)
    }

    /// A stored source's catalog size as an import worker reads it (a
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

    /// Reads a display snapshot without exposing its native metadata records.
    /// The read-lane contention test warms the reader before occupying the writer.
    pub async fn read_library_metadata(&self) -> Result<(), crate::Error> {
        crate::library::snapshot(self)
            .await
            .map(|_| ())
            .map_err(Into::into)
    }
}

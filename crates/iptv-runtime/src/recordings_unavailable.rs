//! Recording, in a runtime built without the `native` feature: unavailable.
//!
//! Capture, the recording host and the loopback playback relay are native
//! (`recordings.rs`, which needs `iptv-net` and workers). A browser host
//! keeps the same API: every recording command answers that recording is
//! unavailable, and viewing uses the identity transport (the host's player
//! fetches the stream itself), so a surface needs no relay and no shared input.
use crate::{Error, Runtime};
use iptv_core::playback::PlaySource;
use iptv_core::recording::{Job, JobId, Request};
use iptv_library::SourceId;
use std::path::{Path, PathBuf};

fn unavailable() -> Error {
    Error::unavailable(iptv_core::message::key("Recording unavailable"))
}

/// The recording state of one owner: nothing to hold without capture.
#[derive(Default)]
pub(crate) struct State;
impl State {
    pub(crate) fn detach_ui(&self) {}
}

pub mod host {
    use super::*;

    pub(crate) fn activate_storage(
        ports: &crate::Ports,
    ) -> Result<(), crate::backup::StartupError> {
        crate::backup::startup(ports)
    }

    pub(crate) fn acquire(_: &crate::Ports) -> std::sync::Arc<State> {
        std::sync::Arc::default()
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn original_player_source(_: &Path, _: &str) -> Option<PlaySource> {
        None
    }

    /// There is no background host to start.
    pub fn start_background_host(_: crate::Ports) -> Result<(), Error> {
        Err(Error::unavailable(iptv_core::message::key(
            "Background recording is unavailable",
        )))
    }

    /// Nothing runs, so nothing has to stop.
    pub fn stop_background_host(_: &Path) -> bool {
        true
    }

    pub fn suspend_viewing(_: &Path) {}

    pub fn resume_viewing(_: &Path) -> bool {
        true
    }

    impl Runtime {
        pub fn shutdown_recording_host(&self) -> bool {
            true
        }
        pub fn recording_background_needed(&self) -> bool {
            false
        }
    }
}

impl Runtime {
    /// No recording host here learns a provider's allowance.
    pub(crate) fn recorded_provider_limit(&self, _source: SourceId) -> Option<u32> {
        None
    }
    pub fn start_recordings(&self) {}
    pub fn sync_recording_contexts(&self) {}
    /// No recording competes for the account's connections.
    pub fn reserve_cast_viewing(&self, _source: SourceId) -> Option<Box<dyn Send>> {
        None
    }
    pub fn reserve_external_viewing(&self, _source: SourceId) -> Option<Box<dyn Send>> {
        None
    }
    pub fn reserve_viewing(&self, _surface: usize, _source: SourceId) -> bool {
        !self.recordings_detached.get()
    }
    pub fn release_viewing(&self, _surface: usize) {}
    pub fn recording_snapshot(&self) -> Vec<Job> {
        Vec::new()
    }
    pub async fn schedule_recording(&self, _request: Request) -> Result<JobId, Error> {
        Err(unavailable())
    }
    pub async fn stop_recording(&self, _id: JobId) -> Result<(), Error> {
        Err(unavailable())
    }
    pub async fn recording_jobs(&self) -> Result<Vec<Job>, Error> {
        Ok(Vec::new())
    }
    /// This build never scheduled recordings, so none were retired.
    pub async fn announce_retired_recordings(&self) -> Result<bool, Error> {
        Ok(false)
    }
    pub async fn retired_recording_schedules(
        &self,
    ) -> Result<Vec<iptv_core::recording::RetiredSchedule>, Error> {
        Ok(Vec::new())
    }
    pub async fn dismiss_retired_recording_schedules(&self) -> Result<(), Error> {
        Ok(())
    }
    pub async fn open_recording(&self, _id: JobId) -> Result<PlaySource, Error> {
        Err(unavailable())
    }
    pub async fn recording_progress_points(
        &self,
    ) -> Result<Vec<(JobId, iptv_core::on_demand::Progress)>, Error> {
        Ok(Vec::new())
    }
    pub async fn recording_progress(
        &self,
        _id: JobId,
    ) -> Result<iptv_core::on_demand::Progress, Error> {
        Err(unavailable())
    }
    pub fn save_recording_progress(
        &self,
        _id: JobId,
        _source: &PlaySource,
        _point: iptv_core::on_demand::Progress,
    ) {
    }
    pub fn close_recording_playback(&self) {}
    pub async fn recording_file(&self, _id: JobId) -> Result<PathBuf, Error> {
        Err(unavailable())
    }
    pub async fn delete_recording(&self, _id: JobId) -> Result<(), Error> {
        Err(unavailable())
    }
    /// No shared input outlives a viewer.
    pub fn retire_viewing_inputs(
        &self,
        _surface: usize,
    ) -> impl std::future::Future<Output = ()> + use<> {
        std::future::ready(())
    }
    pub fn reserve_live_viewing(&self, surface: usize, source: SourceId, _channel: &str) -> bool {
        self.reserve_viewing(surface, source)
    }
    /// The player opens the source itself; nothing on this side owns it.
    pub fn owns_viewing_source(&self, _surface: usize, _source: &PlaySource) -> bool {
        false
    }
    /// The identity transport: the host's player fetches `stream` itself.
    pub fn prepare_live_source(
        &self,
        _surface: usize,
        _source: SourceId,
        _channel: String,
        stream: PlaySource,
        _authority: iptv_guide::playback::ResolutionAuthority,
    ) -> impl std::future::Future<Output = Result<PlaySource, Error>> + use<> {
        std::future::ready(Ok(stream))
    }
    /// The identity transport, as [`Runtime::prepare_live_source`].
    pub fn prepare_independent_source(
        &self,
        _surface: usize,
        _provider: Option<(SourceId, String)>,
        source: PlaySource,
        _authority: Option<iptv_guide::playback::ResolutionAuthority>,
    ) -> impl std::future::Future<Output = Result<PlaySource, Error>> + use<> {
        std::future::ready(Ok(source))
    }
}

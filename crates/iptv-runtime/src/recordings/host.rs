//! Native recording ownership is deliberately outside an application engine.
//! Android can destroy its NSH root while the foreground service retains this
//! host. A process-wide storage lock excludes every competing process.
use super::*;
use crate::store_service::Service;
use std::sync::OnceLock;

static ACTIVATION: Mutex<()> = Mutex::new(());
pub(crate) fn activate_storage(ports: &crate::Ports) -> Result<(), crate::backup::StartupError> {
    let _guard = ACTIVATION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if crate::backup::is_pending(&ports.directory) && !stop_background_host(&ports.directory) {
        return Err(crate::backup::StartupError {
            error: iptv_core::message::key(
                "Recording host did not close; restart Patina before restoring",
            )
            .into(),
            fatal: true,
        });
    }
    crate::backup::startup(ports)
}

static HOSTS: OnceLock<Mutex<HashMap<PathBuf, Arc<State>>>> = OnceLock::new();
fn hosts() -> &'static Mutex<HashMap<PathBuf, Arc<State>>> {
    HOSTS.get_or_init(Mutex::default)
}
#[cfg(feature = "test-support")]
pub(crate) fn original_player_source(directory: &Path, url: &str) -> Option<PlaySource> {
    let state = hosts()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(directory)
        .cloned()?;
    state.original_player_source(url)
}

pub(crate) fn acquire(ports: &crate::Ports) -> Arc<State> {
    if !ports.background_recordings {
        return Arc::default();
    }
    let mut hosts = hosts()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // A running UI and a restarted OS service must always share one account
    // budget and playback capability. Keep this identity even after shutdown.
    if crate::backup::is_pending(&ports.directory)
        && let Some(previous) = hosts.get(&ports.directory)
    {
        previous.stop();
    }
    hosts
        .entry(ports.directory.clone())
        .or_insert_with(|| {
            let state = Arc::new(State::default());
            *state
                .host
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(Arc::new(Service::new(ports.clone())));
            state
        })
        .clone()
}
impl State {
    pub(crate) fn background(&self) -> bool {
        self.host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }
    pub(crate) fn detach_ui(&self) {
        self.cancel_all_preparations();
        let mut data = self.lock();
        data.viewers.clear();
        data.suspended_viewers.clear();
        drop(data);
        self.playback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if !self.background() {
            self.stop();
        }
    }
    fn start(self: &Arc<Self>, ui: Option<&Services>) -> bool {
        // The execution gate serializes UI restart, alarm restart and shutdown.
        // A run keeps its own cancellation token until all captures have joined
        // and its writer/reader handles have been released.
        let mut execution = self.execution();
        if execution.started && !self.lock().stopped {
            return !execution.shutdown.is_canceled();
        }
        let mut host = self
            .host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if execution.started
            && let Some(previous) = host.as_ref()
        {
            let ports = previous.accept().ports().clone();
            *host = Some(Arc::new(Service::new(ports)));
        }
        let services = if let Some(host) = host.as_ref() {
            host.begin();
            let services = host.accept();
            if let Some(ui) = ui {
                *services
                    .shared_settings()
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ui.settings());
            } else if let Ok(text) =
                std::fs::read_to_string(services.directory().join("settings.json"))
                && let Ok((settings, _)) = iptv_settings::Settings::from_json(&text)
            {
                *services
                    .shared_settings()
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(settings));
            }
            services
        } else {
            ui.expect("application recording services").clone()
        };
        drop(host);
        let shutdown = CancelToken::new();
        execution.started = true;
        execution.generation += 1;
        let generation = execution.generation;
        execution.shutdown = shutdown.clone();
        {
            let mut data = self.lock();
            data.stopped = false;
            data.ready = false;
            // A new native run must acknowledge its own OS registration.
            data.wake_reported = false;
            data.published_schedule = None;
            data.guide_check_at = 0;
        }
        let state = self.clone();
        services.clone().spawn(async move {
            run(services.clone(), state.clone(), shutdown).await;
            let closed = if state.background() {
                services.service_retire()
            } else {
                true
            };
            let mut data = state.lock();
            data.stopped = closed;
            if closed {
                data.completed_run = generation;
            }
            drop(data);
            state.changed.notify_all();
        });
        true
    }
}
impl Runtime {
    pub(crate) fn start_recording_host(&self) {
        self.recordings.start(Some(&self.services()));
    }
    /// OS hosts must explicitly stop work before process shutdown/restore.
    /// Accepted future jobs remain durable and are recovered by the next host.
    pub fn shutdown_recording_host(&self) -> bool {
        self.recordings.stop()
    }
    /// [`stop_captures`] for this runtime's recordings.
    pub fn stop_captures(&self, why: EarlyStop) -> usize {
        self.recordings.stop_captures(why)
    }
    pub fn recording_background_needed(&self) -> bool {
        let data = self.recordings.lock();
        data.recurring || data.jobs.iter().any(|job| job.status.active())
    }
}
/// Start without constructing a UI or a rendered player. Call off the native
/// main thread after installing the platform key broker.
pub fn start_background_host(ports: crate::Ports) -> Result<(), Error> {
    if !ports.background_recordings {
        return Err(Error::unavailable("Background recording is unavailable"));
    }
    crate::backup::lock(&ports.directory)
        .map_err(|_| Error::unavailable("Recording storage already in use"))?;
    // Serialize native restarts with catalog activation and handle closure.
    let _activation = ACTIVATION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let state = acquire(&ports);
    if !state.execution().started || crate::backup::is_pending(&ports.directory) {
        crate::backup::startup(&ports)
            .map_err(|_| Error::unavailable("Recording storage recovery failed"))?;
    }
    if !state.start(None) {
        return Err(Error::unavailable(
            "Restart Patina before scheduling a recording",
        ));
    }
    Ok(())
}
pub fn stop_background_host(directory: &Path) -> bool {
    let state = hosts()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(directory)
        .cloned();
    state.is_none_or(|state| state.stop())
}

/// Why a host stops running captures early ([`stop_captures`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EarlyStop {
    /// The TV turned off, and its Low Power Standby cuts the network.
    TvOff,
    /// The operating system's limit on background work was reached
    /// (Android's six hours a day for a data-sync service).
    BackgroundLimit,
}

impl EarlyStop {
    /// The message the recording keeps.
    pub fn reason(self) -> &'static str {
        match self {
            Self::TvOff => "Recording stopped when the TV was turned off",
            Self::BackgroundLimit => {
                "Recording stopped: Android's background time limit was reached"
            }
        }
    }
}

/// Stops every capture running for the data directory `directory`, for
/// `why`: each closes its file and is kept as a partial recording that
/// gives the reason. Returns how many were running.
pub fn stop_captures(directory: &Path, why: EarlyStop) -> usize {
    let state = hosts()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(directory)
        .cloned();
    state.map_or(0, |state| state.stop_captures(why))
}

impl State {
    pub(crate) fn stop_captures(&self, why: EarlyStop) -> usize {
        let mut data = self.lock();
        let running: Vec<_> = data
            .active
            .iter()
            .map(|(id, (_, stop))| (*id, stop.clone()))
            .collect();
        for (id, stop) in &running {
            data.stop_reasons.insert(*id, why.reason());
            stop.cancel();
        }
        running.len()
    }
}

/// Called only after native mobile players have closed their connections.
pub fn suspend_viewing(directory: &Path) {
    if let Some(state) = hosts()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(directory)
        .cloned()
    {
        state.cancel_all_preparations();
        let mut data = state.lock();
        let viewers = std::mem::take(&mut data.viewers);
        data.suspended_viewers.extend(viewers);
    }
}
/// Reserve the complete returning set before any native player reconnects.
/// If it no longer fits, rendered players stay mounted and emit a retry event.
pub fn resume_viewing(directory: &Path) -> bool {
    let state = hosts()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(directory)
        .cloned();
    let Some(state) = state else { return true };
    let mut data = state.lock();
    let mut wanted: Vec<_> = data
        .suspended_viewers
        .iter()
        .map(|(surface, lease)| (*surface, lease.clone()))
        .collect();
    wanted.sort_by_key(|(surface, _)| *surface);
    let mut restored = Vec::new();
    for (surface, lease) in wanted {
        if !data.capacity_with(
            lease.source,
            Some(surface),
            lease.input.as_ref().map(|input| input.id),
        ) {
            for surface in restored {
                data.viewers.remove(&surface);
            }
            return false;
        }
        data.viewers.insert(surface, lease);
        restored.push(surface);
    }
    data.suspended_viewers.clear();
    true
}

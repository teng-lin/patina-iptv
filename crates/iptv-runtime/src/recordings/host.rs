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
    crate::backup::startup(ports)
}

static HOSTS: OnceLock<Mutex<HashMap<PathBuf, Arc<State>>>> = OnceLock::new();
fn hosts() -> &'static Mutex<HashMap<PathBuf, Arc<State>>> {
    HOSTS.get_or_init(Mutex::default)
}
pub(crate) fn acquire(ports: &crate::Ports) -> Arc<State> {
    if !ports.background_recordings {
        return Arc::default();
    }
    let mut hosts = hosts()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // A pending restore must not reopen yesterday's catalogue handles.
    if crate::backup::is_pending(&ports.directory) {
        if let Some(previous) = hosts.get(&ports.directory).cloned() {
            if previous.stop() {
                hosts.remove(&ports.directory);
            } else {
                return previous;
            }
        }
    }
    if hosts
        .get(&ports.directory)
        .is_some_and(|state| state.lock().stopped)
    {
        hosts.remove(&ports.directory);
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
    fn start(self: &Arc<Self>, ui: Option<&Services>) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let host = self
            .host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let services = if let Some(host) = host {
            host.begin();
            let services = host.accept();
            if let Some(ui) = ui {
                *services
                    .shared_settings()
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ui.settings());
            } else {
                // Native host initialization runs off the main thread. Invalid
                // settings are left untouched; a UI may later repair them.
                if let Ok(text) =
                    std::fs::read_to_string(services.directory().join("settings.json"))
                    && let Ok((settings, _)) = iptv_settings::Settings::from_json(&text)
                {
                    *services
                        .shared_settings()
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some(Arc::new(settings));
                }
            }
            services
        } else {
            ui.expect("application recording services").clone()
        };
        let state = self.clone();
        services.clone().spawn(async move {
            run(services.clone(), state.clone()).await;
            let closed = if state.background() {
                services.service_retire()
            } else {
                true
            };
            state.lock().stopped = closed;
            state.changed.notify_all();
        });
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
    let state = acquire(&ports);
    if !state.started.load(Ordering::SeqCst) {
        activate_storage(&ports)
            .map_err(|_| Error::unavailable("Recording storage recovery failed"))?;
        state.start(None);
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

/// Called only after native mobile players have closed their connections.
pub fn suspend_viewing(directory: &Path) {
    if let Some(state) = hosts()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(directory)
        .cloned()
    {
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
        if !data.capacity(lease.source, Some(surface)) {
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

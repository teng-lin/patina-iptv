//! App-open DVR. Durable intent is committed before acknowledgement; media workers
//! have no rendered player and use the same connection budget as viewing surfaces.
pub mod host;
mod rules;
#[cfg(test)]
mod tests;
use crate::{Error, Runtime, store_service::Services};
use iptv_core::{
    playback::PlaySource,
    recording::{Job, JobId, MAX_BYTES, Request, Status},
};
use iptv_import::CancelToken;
use iptv_library::SourceId;
use iptv_net::playback::PlaybackPolicy;
use iptv_providers::{ContentId, model::SourceConfig};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs().min(i64::MAX as u64) as i64)
}

// Private in-memory identities; neither keys nor credentials are logged/persisted.
#[derive(Clone, PartialEq, Eq)]
enum Account {
    Xtream(String, String),
    Stalker(Vec<String>, String),
    // M3U cannot establish account equivalence. Conservatively share a budget.
    Unknown,
}
fn account(config: &SourceConfig) -> Account {
    match config {
        SourceConfig::Xtream(c) => Account::Xtream(
            iptv_providers::xtream::normalize_server_url(&c.server_url)
                .unwrap_or_else(|_| c.server_url.clone()),
            c.username.expose().into(),
        ),
        SourceConfig::Stalker(c) => {
            let mut endpoints = iptv_providers::stalker::endpoint_candidates(&c.portal_url)
                .unwrap_or_else(|_| vec![c.portal_url.clone()]);
            endpoints.sort();
            Account::Stalker(endpoints, c.mac.expose().into())
        }
        SourceConfig::M3u(_) => Account::Unknown,
    }
}
#[derive(Clone)]
struct Lease {
    source: i64,
    account: Account,
    limit: u32,
}
fn same_account(a: &Account, b: &Account) -> bool {
    match (a, b) {
        (Account::Unknown, _) | (_, Account::Unknown) => true,
        (Account::Stalker(a, mac_a), Account::Stalker(b, mac_b)) => {
            mac_a == mac_b && a.iter().any(|url| b.contains(url))
        }
        _ => a == b,
    }
}
#[derive(Default)]
struct Data {
    accounts: HashMap<i64, Account>,
    limits: HashMap<i64, u32>,
    provider_limits: HashMap<i64, u32>,
    viewers: HashMap<usize, Lease>,
    suspended_viewers: HashMap<usize, Lease>,
    active: HashMap<JobId, (Lease, CancelToken)>,
    jobs: Vec<Job>,
    recurring: bool,
    ready: bool,
    stopped: bool,
}
impl Data {
    fn lease(&self, source: i64) -> Lease {
        Lease {
            source,
            account: self
                .accounts
                .get(&source)
                .cloned()
                .unwrap_or(Account::Unknown),
            limit: self
                .provider_limits
                .get(&source)
                .copied()
                .unwrap_or(u32::MAX),
        }
    }
    fn capacity(&self, source: i64, replacing: Option<usize>) -> bool {
        let wanted = self.lease(source);
        let same = |lease: &Lease| same_account(&lease.account, &wanted.account);
        // Existing leases retain their original account and limit when a playlist is edited.
        let leases = self
            .viewers
            .iter()
            .filter(|(surface, _)| Some(**surface) != replacing)
            .map(|(_, lease)| lease)
            .chain(self.active.values().map(|(lease, _)| lease))
            .filter(|lease| same(lease));
        let (mut used, mut limit) = (
            0,
            wanted
                .limit
                .min(self.limits.get(&source).copied().unwrap_or(1)),
        );
        for lease in leases {
            used += 1;
            limit = limit.min(lease.limit);
        }
        for (id, account) in &self.accounts {
            if same_account(account, &wanted.account) {
                limit = limit
                    .min(self.lease(*id).limit)
                    .min(self.limits.get(id).copied().unwrap_or(1));
            }
        }
        used < limit
    }
}
struct ContextEntry {
    pointer: usize,
    config: SourceConfig,
    value: iptv_guide::playback::PlaybackContext,
}
#[derive(Default)]
pub(crate) struct State {
    host: Mutex<Option<Arc<crate::store_service::Service>>>,
    data: Mutex<Data>,
    changed: Condvar,
    started: AtomicBool,
    shutdown: CancelToken,
    contexts: Mutex<HashMap<i64, ContextEntry>>,
    media_ops: tokio::sync::Mutex<()>,
    playback: Mutex<Option<(JobId, iptv_net::recording_file::RecordingFile)>>,
}
impl State {
    fn lock(&self) -> std::sync::MutexGuard<'_, Data> {
        self.data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub(crate) fn stop(&self) -> bool {
        if !self.started.load(Ordering::SeqCst) {
            return true;
        }
        self.shutdown.cancel();
        self.changed
            .wait_timeout_while(self.lock(), Duration::from_secs(15), |d| !d.stopped)
            .map(|(data, _)| data.stopped)
            .unwrap_or(false)
    }
}

/// Media lives beside the catalogue directory, so catalogue restore cannot delete
/// recordings. This app-managed directory is private and excluded from OS backup.
pub fn media_directory(root: &Path) -> PathBuf {
    let mut name = root.file_name().unwrap_or_default().to_os_string();
    name.push("-recordings");
    root.with_file_name(name)
}
fn path(services: &Services, id: JobId, partial: bool) -> PathBuf {
    media_directory(services.directory()).join(format!(
        "{}.{}",
        id.0,
        if partial { "part" } else { "ts" }
    ))
}

// Android app sandboxes prohibit hard links. Linux's no-replace rename keeps
// finalization atomic without overwriting previously acknowledged media.
fn finalize_name(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(any(target_os = "android", target_os = "linux"))]
    {
        use std::os::unix::ffi::OsStrExt;
        let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
        let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
        // SAFETY: both C strings stay alive through the synchronous syscall;
        // AT_FDCWD and RENAME_NOREPLACE (1) are Linux kernel constants.
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                1u32,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "android", target_os = "linux")))]
    {
        std::fs::hard_link(from, to)?;
        std::fs::remove_file(from)
    }
}

/// Called on file workers only. A stopped partial remains readable even when
/// the filesystem could not publish its final name; active jobs never use it.
fn retained_media(services: &Services, id: JobId, partial: bool) -> Option<(PathBuf, u64)> {
    [false, true].into_iter().find_map(|is_partial| {
        if is_partial && !partial {
            return None;
        }
        let file = path(services, id, is_partial);
        let metadata = std::fs::symlink_metadata(&file).ok()?;
        let bytes = metadata.len() / 188 * 188;
        (metadata.is_file() && !metadata.file_type().is_symlink() && bytes > 0)
            .then_some((file, bytes))
    })
}

impl Runtime {
    pub fn start_recordings(&self) {
        self.start_recording_host();
    }
    /// Synchronize imported sessions/limits without sharing UI-owned Rc state.
    pub fn sync_recording_contexts(&self) {
        if self.recordings_detached.get() {
            return;
        }
        if let Some(host) = self
            .recordings
            .host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            *host
                .accept()
                .shared_settings()
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(self.services().settings());
        }
        let mut data = self.recordings.lock();
        if let (Some(source), Some(limit)) = (self.shown.get(), self.account_streams.get()) {
            data.provider_limits.insert(source.0, limit);
        }
        for (source, limit) in self.limits.borrow().iter() {
            if let Some(limit) = limit {
                data.provider_limits.insert(source.0, *limit);
            }
        }
        let mut contexts = self
            .recordings
            .contexts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut keep = |source: SourceId, schedule: &iptv_guide::EpgSchedule| {
            let Some(context) = schedule.shared_playback() else {
                return;
            };
            let Some(config) = crate::library::source_details(self, source).map(|s| s.config)
            else {
                return;
            };
            let pointer = std::rc::Rc::as_ptr(&context) as usize;
            if let Some(entry) = contexts
                .get_mut(&source.0)
                .filter(|entry| entry.pointer == pointer)
            {
                // Reauthentication updates the shared session, but cannot relabel an
                // old context with credentials edited after that import.
                entry.value = (*context).clone();
            } else {
                contexts.insert(
                    source.0,
                    ContextEntry {
                        pointer,
                        config,
                        value: (*context).clone(),
                    },
                );
            }
        };
        for (source, schedule) in self.parts.borrow().iter() {
            keep(*source, schedule);
        }
        if let Some(single) = self.single.borrow().as_ref() {
            keep(single.source, &single.schedule);
            if let Some(limit) = single.limit {
                data.provider_limits.insert(single.source.0, limit);
            }
        }
    }
    /// Called before changing a viewing surface, including multiview. A retry of
    /// an existing surface retains its reservation; PiP adds no reservation.
    pub fn reserve_viewing(&self, surface: usize, source: SourceId) -> bool {
        if self.recordings_detached.get() {
            return false;
        }
        let mut data = self.recordings.lock();
        if !data.active.is_empty() && !data.capacity(source.0, Some(surface)) {
            return false;
        }
        let lease = data.lease(source.0);
        data.suspended_viewers.remove(&surface);
        data.viewers.insert(surface, lease);
        true
    }
    pub fn release_viewing(&self, surface: usize) {
        if self.recordings_detached.get() {
            return;
        }
        let mut data = self.recordings.lock();
        data.viewers.remove(&surface);
        data.suspended_viewers.remove(&surface);
    }
    pub fn recording_snapshot(&self) -> Vec<Job> {
        self.recordings.lock().jobs.clone()
    }

    pub async fn schedule_recording(&self, request: Request) -> Result<JobId, Error> {
        request.validate(now()).map_err(Error::unavailable)?;
        if self.recordings.shutdown.is_cancelled() || !self.services().is_current() {
            return Err(Error::unavailable(
                "Restart Patina before scheduling a recording",
            ));
        }
        self.sync_recording_contexts();
        self.start_recordings();
        // Startup reconciliation must precede newly accepted record-now jobs.
        for _ in 0..100 {
            if self.recordings.lock().ready {
                break;
            }
            self.services()
                .spawn(async { tokio::time::sleep(Duration::from_millis(20)).await })
                .await
                .map_err(|_| Error::unavailable("Recording service unavailable"))?;
        }
        if !self.recordings.lock().ready {
            return Err(Error::unavailable("Recording service unavailable"));
        }
        let duplicate = request.clone();
        if let Some(id) = self
            .services()
            .read(move |store| {
                Ok(store
                    .recording_jobs()?
                    .into_iter()
                    .find(|j| {
                        j.status.active()
                            && j.source_current
                            && j.request.source == duplicate.source
                            && j.request.channel == duplicate.channel
                            && j.request.start == duplicate.start
                            && j.request.end == duplicate.end
                    })
                    .map(|j| j.id))
            })
            .await?
        {
            return Ok(id);
        }
        refresh_accounts(&self.services(), &self.recordings)
            .await
            .map_err(|_| Error::unavailable("Recording sources unavailable"))?;
        if request.start <= now() && !self.recordings.lock().capacity(request.source, None) {
            return Err(Error::unavailable(
                "No provider connection available; stop another stream or check Streams at once",
            ));
        }
        let mut random = [0u8; 8];
        getrandom::fill(&mut random)
            .map_err(|_| Error::unavailable("Recording identity unavailable"))?;
        let id = JobId((i64::from_ne_bytes(random) & i64::MAX).max(1));
        let id = self
            .services()
            .call(move |store| store.schedule_recording(id, &request, now()))
            .await
            .map_err(|_| {
                Error::unavailable(
                    "Recording could not be scheduled; check the channel and recording times",
                )
            })?;
        self.acknowledge_recording(id).await?;
        Ok(id)
    }
    pub(crate) async fn acknowledge_recording(&self, id: JobId) -> Result<(), Error> {
        if let Some(job) = self
            .services()
            .read(move |store| store.recording_job(id))
            .await?
        {
            let next = job.request.start;
            let mut data = self.recordings.lock();
            let active = next <= now() || !data.active.is_empty();
            data.jobs.retain(|existing| existing.id != id);
            data.jobs.push(job);
            let earliest = data
                .jobs
                .iter()
                .filter(|j| j.status == Status::Scheduled)
                .map(|j| j.request.start)
                .min()
                .unwrap_or(next);
            drop(data);
            (self.services().ports().recording_schedule)(earliest, active);
        }
        Ok(())
    }
    /// Cancels a future job, or requests that an active recording stop and retain media.
    pub async fn update_recording(&self, id: JobId, request: Request) -> Result<(), Error> {
        self.parental_check(&iptv_core::parental::Target::live(
            request.source,
            request.channel.clone(),
        ))
        .await
        .map_err(|_| Error::unavailable("Parental access is locked"))?;
        if !self
            .services()
            .call(move |store| store.update_recording(id, &request, now()))
            .await?
        {
            return Err(Error::unavailable(
                "This recording has already started or was cancelled",
            ));
        }
        self.acknowledge_recording(id).await?;
        Ok(())
    }

    pub async fn stop_recording(&self, id: JobId) -> Result<(), Error> {
        if let Some(job) = self
            .services()
            .read(move |store| store.recording_job(id))
            .await?
        {
            if job.status == Status::Scheduled && job.bytes > 0 {
                let bytes = job.bytes;
                if self
                    .services()
                    .call(move |store| {
                        store.transition_recording(
                            id,
                            Status::Scheduled,
                            Status::Partial,
                            bytes,
                            "Stopped by you",
                        )
                    })
                    .await?
                {
                    // Invalidate the scheduler's stale snapshot before publishing
                    // the spool: no capture may append during this rename.
                    let mut stopped = job;
                    stopped.status = Status::Partial;
                    recover_media(&self.services(), &stopped).await;
                    return Ok(());
                }
            }
        }
        if let Some((_, stop)) = self.recordings.lock().active.get(&id) {
            stop.cancel();
        }
        self.services()
            .call(move |store| {
                store.transition_recording(
                    id,
                    Status::Scheduled,
                    Status::Cancelled,
                    0,
                    "Cancelled",
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }
    pub async fn recording_jobs(&self) -> Result<Vec<Job>, Error> {
        let mut jobs = self.services().read(|store| store.recording_jobs()).await?;
        let services = self.services();
        jobs = self
            .services()
            .spawn_blocking(move || {
                for job in &mut jobs {
                    if job.status == Status::Recording {
                        job.bytes = std::fs::symlink_metadata(path(&services, job.id, true))
                            .ok()
                            .filter(|m| m.is_file() && !m.file_type().is_symlink())
                            .map_or(0, |m| m.len());
                        if job.bytes > 0 {
                            job.message = "Recording started".into();
                        }
                    }
                }
                jobs
            })
            .await
            .map_err(|_| Error::unavailable("Recordings unavailable"))?;
        Ok(jobs)
    }
    pub async fn open_recording(&self, id: JobId) -> Result<PlaySource, Error> {
        let _operation = self.recordings.media_ops.lock().await;
        let file = self.recording_file(id).await?;
        let relay = self
            .services()
            .spawn(async move { iptv_net::recording_file::RecordingFile::start(file) })
            .await
            .map_err(|_| Error::unavailable("Recording unavailable"))?
            .map_err(|_| Error::unavailable("Recording unavailable"))?;
        let mut source = PlaySource::new(relay.url.clone());
        source.public_only = false;
        *self
            .recordings
            .playback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((id, relay));
        Ok(source)
    }
    pub async fn recording_progress_points(
        &self,
    ) -> Result<Vec<(JobId, iptv_core::on_demand::Progress)>, Error> {
        Ok(self
            .services()
            .read(|store| store.recording_progress_points())
            .await?)
    }
    pub async fn recording_progress(
        &self,
        id: JobId,
    ) -> Result<iptv_core::on_demand::Progress, Error> {
        Ok(self
            .services()
            .call(move |store| store.recording_progress(id))
            .await?)
    }
    /// Only the active file capability may update its progress. Queues in FIFO
    /// order before returning, including the final write during UI disposal.
    pub fn save_recording_progress(
        &self,
        id: JobId,
        source: &PlaySource,
        point: iptv_core::on_demand::Progress,
    ) {
        let playback = self
            .recordings
            .playback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if playback
            .as_ref()
            .is_some_and(|(playing, relay)| *playing == id && relay.url == source.url)
        {
            drop(
                self.services()
                    .call(move |store| store.save_recording_progress(id, point, now())),
            );
        }
    }
    pub fn close_recording_playback(&self) {
        self.recordings
            .playback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
    pub async fn recording_file(&self, id: JobId) -> Result<PathBuf, Error> {
        let job = self
            .services()
            .read(move |store| store.recording_job(id))
            .await?
            .ok_or_else(|| Error::unavailable("Recording unavailable"))?;
        self.parental_check(&iptv_core::parental::Target::live(
            job.request.source,
            job.request.channel.as_str(),
        ))
        .await
        .map_err(|_| Error::unavailable("Unlock this recording with the parental PIN"))?;
        if !matches!(job.status, Status::Completed | Status::Partial) || job.bytes == 0 {
            return Err(Error::unavailable("This recording has no playable media"));
        }
        let services = self.services();
        self.services()
            .spawn_blocking(move || retained_media(&services, id, job.status == Status::Partial))
            .await
            .ok()
            .flatten()
            .map(|(file, _)| file)
            .ok_or_else(|| Error::unavailable("The recording file is missing"))
    }
    pub async fn delete_recording(&self, id: JobId) -> Result<(), Error> {
        // Admission and deletion stay mutually exclusive across store/file awaits.
        // Cancellation drops this guard too, so a failed operation cannot strand it.
        let _operation = self.recordings.media_ops.lock().await;
        if self
            .recordings
            .playback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|(playing, _)| *playing == id)
        {
            return Err(Error::unavailable(
                "Stop playback before deleting this recording",
            ));
        }
        let job = self
            .services()
            .read(move |store| store.recording_job(id))
            .await?
            .ok_or_else(|| Error::unavailable("Recording unavailable"))?;
        if job.status.active() {
            return Err(Error::unavailable("Stop or cancel this recording first"));
        }
        let services = self.services();
        self.services()
            .spawn_blocking(move || {
                for partial in [false, true] {
                    match std::fs::remove_file(path(&services, id, partial)) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(_) => return Err(Error::unavailable("Recording could not be deleted")),
                    }
                }
                Ok(())
            })
            .await
            .map_err(|_| Error::unavailable("Recording could not be deleted"))??;
        self.services()
            .call(move |store| store.remove_recording(id))
            .await?;
        Ok(())
    }
}

async fn refresh_accounts(
    services: &Services,
    state: &State,
) -> Result<(), crate::model::StoreError> {
    let sources = services
        .read(|store| {
            store
                .sources()?
                .into_iter()
                .map(|source| store.source(source.id))
                .collect::<iptv_store_sqlite::Result<Vec<_>>>()
        })
        .await?;
    let settings = services.settings();
    let mut data = state.lock();
    data.accounts.clear();
    data.limits.clear();
    for source in sources.into_iter().flatten() {
        let limit = match settings.get_in(
            "playlists.max_connections",
            &iptv_settings::Target::playlist(source.id.0.to_string()),
        ) {
            Ok(iptv_settings::Value::Int(n)) => n.clamp(1, 20) as u32,
            _ => 1,
        };
        data.accounts
            .insert(source.id.0, account(&source.source.config));
        data.limits.insert(source.id.0, limit);
    }
    Ok(())
}
async fn transition(services: &Services, job: &Job, to: Status, bytes: u64, message: &'static str) {
    let (id, from) = (job.id, job.status);
    let _ = services
        .call(move |store| store.transition_recording(id, from, to, bytes, message))
        .await;
}
async fn recover_media(services: &Services, job: &Job) {
    let p = path(services, job.id, true);
    let complete = path(services, job.id, false);
    let files = services.clone();
    let id = job.id;
    let bytes = services
        .spawn_blocking(move || {
            let _ = finalize_name(&p, &complete);
            retained_media(&files, id, true).map_or(0, |(_, bytes)| bytes)
        })
        .await
        .unwrap_or(0);
    if job.status == Status::Partial && bytes > 0 {
        return;
    }
    transition(
        services,
        job,
        if bytes > 0 {
            Status::Partial
        } else {
            Status::Failed
        },
        bytes,
        "Interrupted when the app closed",
    )
    .await;
}
async fn recover(services: &Services) -> Result<(), crate::model::StoreError> {
    for job in services.read(|store| store.recording_jobs()).await? {
        if job.status == Status::Recording && job.request.end > now() {
            let partial = path(services, job.id, true);
            let bytes = services
                .spawn_blocking(move || {
                    std::fs::symlink_metadata(partial)
                        .ok()
                        .filter(|m| m.is_file() && !m.file_type().is_symlink())
                        .map_or(0, |m| m.len() / 188 * 188)
                })
                .await
                .unwrap_or(0);
            transition(
                services,
                &job,
                Status::Scheduled,
                bytes,
                "Resuming interrupted recording",
            )
            .await;
        } else if matches!(job.status, Status::Recording | Status::Partial) {
            recover_media(services, &job).await;
        } else if job.status == Status::Scheduled && job.request.end <= now() {
            transition(
                services,
                &job,
                Status::Missed,
                0,
                "The recording window elapsed while the host was unavailable",
            )
            .await;
        }
    }
    Ok(())
}
async fn run(services: Services, state: Arc<State>) {
    let mut retry = Duration::from_millis(250);
    loop {
        if !services.is_current() || state.shutdown.is_cancelled() {
            return;
        }
        if recover(&services).await.is_ok() && refresh_accounts(&services, &state).await.is_ok() {
            break;
        }
        tokio::select! {
            _ = state.shutdown.cancelled() => return,
            _ = tokio::time::sleep(retry) => {},
        }
        retry = (retry * 2).min(Duration::from_secs(5));
    }
    state.lock().ready = true;
    let mut tasks = tokio::task::JoinSet::new();
    let mut last_schedule = None;
    let mut rules_at = 0;
    let mut guides_at = 0;
    loop {
        while tasks.try_join_next().is_some() {}
        if !services.is_current() || state.shutdown.is_cancelled() {
            state.shutdown.cancel();
            while tasks.join_next().await.is_some() {}
            break;
        }
        if refresh_accounts(&services, &state).await.is_err() {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }
        let Ok(jobs) = services.read(|store| store.recording_jobs()).await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        if now() >= guides_at {
            if let Ok(rules) = services.read(|store| store.recording_rules()).await {
                let sources: std::collections::BTreeSet<_> = rules
                    .into_iter()
                    .filter(|rule| {
                        rule.enabled && rule.repeat == iptv_core::recording::Repeat::Series
                    })
                    .map(|rule| SourceId(rule.request.source))
                    .collect();
                for source in sources {
                    let work = services.clone();
                    let cancel = state.shutdown.clone();
                    services.submit(false, move || {
                        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
                        work.block_on(crate::imports::guide_pipeline(
                            &work,
                            source,
                            &tx,
                            crate::imports::GuideRead::Check,
                            &cancel,
                        ));
                    });
                }
            }
            guides_at = now().saturating_add(30 * 60);
        }
        if now() >= rules_at {
            if let Ok(rules) = services.read(|store| store.recording_rules()).await {
                state.lock().recurring = rules.iter().any(|rule| rule.enabled);
            }
            let _ = rules::materialize(&services).await;
            rules_at = now().saturating_add(30);
        }
        let next = jobs
            .iter()
            .filter(|j| j.status == Status::Scheduled)
            .map(|j| j.request.start)
            .min()
            .unwrap_or(0);
        let next = if state.lock().recurring {
            if next > 0 {
                next.min(guides_at)
            } else {
                guides_at
            }
        } else {
            next
        };
        let schedule = (
            next,
            !state.lock().active.is_empty()
                || jobs.iter().any(|j| {
                    j.status == Status::Scheduled
                        && j.request.start <= now()
                        && j.request.end > now()
                }),
        );
        if last_schedule != Some(schedule) {
            (services.ports().recording_schedule)(schedule.0, schedule.1);
            last_schedule = Some(schedule);
        }
        state.lock().jobs = jobs.clone();
        for mut job in jobs {
            if job.status == Status::Recording && !state.lock().active.contains_key(&job.id) {
                recover_media(&services, &job).await;
                continue;
            }
            if job.status != Status::Scheduled || job.request.start > now() {
                continue;
            }
            if job.request.end <= now() {
                transition(
                    &services,
                    &job,
                    Status::Missed,
                    0,
                    "The scheduled recording window was missed",
                )
                .await;
                continue;
            }
            let stop = CancelToken::new();
            let reserved = {
                let mut data = state.lock();
                if data.active.len() >= 4 || !data.capacity(job.request.source, None) {
                    false
                } else {
                    let lease = data.lease(job.request.source);
                    data.active.insert(job.id, (lease, stop.clone()));
                    true
                }
            };
            if !reserved {
                transition(
                    &services,
                    &job,
                    Status::Failed,
                    0,
                    "No provider connection available; check Streams at once",
                )
                .await;
                continue;
            }
            let id = job.id;
            let claim = job.clone();
            match services
                .call(move |store| store.claim_recording(&claim))
                .await
            {
                Ok(true) => {}
                _ => {
                    state.lock().active.remove(&id);
                    continue;
                }
            }
            job.status = Status::Recording;
            let services = services.clone();
            let state = state.clone();
            tasks.spawn(async move {
                struct Release(Arc<State>, JobId);
                impl Drop for Release {
                    fn drop(&mut self) {
                        self.0.lock().active.remove(&self.1);
                    }
                }
                let _release = Release(state.clone(), job.id);
                record(&services, &state, &job, &stop).await;
            });
        }
        tokio::select! {
            _ = state.shutdown.cancelled() => {},
            _ = tokio::time::sleep(Duration::from_secs(1)) => {},
        }
    }
}

async fn resolve(
    services: &Services,
    state: &State,
    job: &Job,
    stop: &CancelToken,
) -> Result<PlaySource, &'static str> {
    let (source, channel) = (
        SourceId(job.request.source),
        ContentId::from_stored(job.request.channel.clone()),
    );
    let id = job.id;
    let (source, channel) = services
        .read(move |store| {
            if !store.source_policy(source)?.enabled {
                return Err(iptv_store_sqlite::StoreError::Unavailable(
                    "Playlist disabled",
                ));
            }
            let result = (store.source(source)?, store.channel(source, &channel)?);
            if !store.recording_source_is_current(id)? {
                return Err(iptv_store_sqlite::StoreError::Unavailable(
                    "Playlist unavailable",
                ));
            }
            Ok(result)
        })
        .await
        .map_err(|_| "Channel unavailable")?;
    let source = source.ok_or("Playlist unavailable")?;
    let channel = channel
        .filter(|c| !c.blocked)
        .ok_or("Channel unavailable or blocked")?
        .channel;
    let mut context = state
        .contexts
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&job.request.source)
        .filter(|entry| entry.config == source.source.config)
        .map(|entry| entry.value.clone())
        .or_else(|| crate::playback::stored_context(&source.source.config));
    if context.is_none()
        && let SourceConfig::Stalker(credentials) = &source.source.config
    {
        let client = iptv_net::NetClient::new(crate::settings::net_config(&services.settings()))
            .map_err(|_| "Network unavailable")?;
        let session = iptv_import::stalker_session(&client, credentials, stop)
            .await
            .map_err(|_| "Portal authentication unavailable")?;
        context = Some(iptv_guide::playback::PlaybackContext::stalker(session));
    }
    let resolution = iptv_guide::playback::resolve(
        context.as_ref(),
        &channel,
        None,
        iptv_guide::playback::Clock {
            now: now(),
            local_offset_secs: 0,
        },
    );
    let mut stream = match resolution {
        iptv_guide::playback::Resolution::Play(source) => source,
        iptv_guide::playback::Resolution::StalkerLink(link) => {
            let iptv_guide::playback::StalkerLink {
                session: mut portal,
                cmd,
                channel_headers,
            } = *link;
            let client =
                iptv_net::NetClient::new(crate::settings::net_config(&services.settings()))
                    .map_err(|_| "Network unavailable")?;
            let url = iptv_import::stalker_link(&client, &mut portal, &cmd, false, stop)
                .await
                .map_err(|_| "Portal stream unavailable")?;
            iptv_guide::playback::StalkerLink::source(&portal, url, &channel_headers)
        }
        _ => return Err("Refresh the playlist before recording this channel"),
    };
    let config = crate::settings::net_config(&services.settings());
    stream.default_header("User-Agent", &config.user_agent);
    Ok(stream)
}

async fn record(services: &Services, state: &State, job: &Job, stop: &CancelToken) {
    let partial = path(services, job.id, true);
    let destination = path(services, job.id, false);
    let directory = media_directory(services.directory());
    let exclude = services.ports().exclude_from_backup;
    let reserved = state.lock().active.len() as u64 * MAX_BYTES;
    let prepared = services
        .spawn_blocking(move || -> std::io::Result<()> {
            crate::storage::protect_directory(&directory, exclude)?;
            let mut total = reserved;
            for entry in std::fs::read_dir(&directory)? {
                let metadata = entry?.metadata()?;
                total = total.saturating_add(metadata.len());
            }
            if total > 32 * 1024 * 1024 * 1024 {
                return Err(std::io::ErrorKind::StorageFull.into());
            }
            Ok(())
        })
        .await;
    let had_gap = Arc::new(AtomicBool::new(
        job.request.start < now().saturating_sub(5) || job.bytes > 0,
    ));
    let gap = had_gap.clone();
    let result = async {
        if !matches!(prepared, Ok(Ok(()))) {
            return Err("Recording storage unavailable");
        }
        let mut retry = Duration::from_secs(1);
        loop {
            let source = match resolve(services, state, job, stop).await {
                Ok(source) => source,
                Err(_) => {
                    gap.store(true, Ordering::SeqCst);
                    tokio::time::sleep(retry).await;
                    retry = (retry * 2).min(Duration::from_secs(15));
                    continue;
                }
            };
            let net = crate::settings::net_config(&services.settings());
            let policy = PlaybackPolicy {
                public_only: source.public_only,
                extra_roots_der: net.extra_roots_der,
                #[cfg(debug_assertions)]
                allow_private_fixture: net.allow_private_fixture,
            };
            match iptv_net::recording::capture_resume(
                &source.url,
                &source.headers,
                policy,
                &partial,
                MAX_BYTES,
            )
            .await
            {
                Err(iptv_net::recording::Failure::Network | iptv_net::recording::Failure::Gap) => {
                    gap.store(true, Ordering::SeqCst);
                    tokio::time::sleep(retry).await;
                    retry = (retry * 2).min(Duration::from_secs(15));
                }
                Err(error) => return Err(error.message()),
                Ok(()) => return Ok(()),
            }
        }
    };
    let remaining = job.request.end.saturating_sub(now()).max(0) as u64;
    let outcome = tokio::select! {
        biased;
        _ = state.shutdown.cancelled() => Err("Interrupted when the app closed"),
        _ = stop.cancelled() => Ok("Stopped by you"),
        _ = async {
            let limit = tokio::time::Instant::now() + Duration::from_secs(remaining);
            while now() < job.request.end && tokio::time::Instant::now() < limit {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        } => Ok("Recording finished"),
        result = result => result.map(|_| "Recording finished"),
    };
    if state.shutdown.is_cancelled() && state.background() && job.request.end > now() {
        let bytes = std::fs::symlink_metadata(&partial).map_or(0, |m| m.len() / 188 * 188);
        transition(
            services,
            job,
            Status::Scheduled,
            bytes,
            "Resuming interrupted recording",
        )
        .await;
        return;
    }
    let outcome = if outcome.is_ok() && had_gap.load(Ordering::SeqCst) {
        Err("Recording retained with a gap after interruption")
    } else {
        outcome
    };
    // capture uses synchronous file writes on this native worker: dropping its
    // future cannot leave an asynchronous write racing finalization/deletion.
    let finalized = services
        .spawn_blocking(move || -> std::io::Result<u64> {
            let metadata = match std::fs::symlink_metadata(&partial) {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
                Err(e) => return Err(e),
            };
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
            let bytes = metadata.len() / 188 * 188;
            if bytes == 0 {
                std::fs::remove_file(partial)?;
                return Ok(0);
            }
            let file = std::fs::OpenOptions::new().write(true).open(&partial)?;
            file.set_len(bytes)?;
            file.sync_all()?;
            // Never replace a previous completed file, even after a lost acknowledgement.
            finalize_name(&partial, &destination)?;
            #[cfg(unix)]
            std::fs::File::open(
                destination
                    .parent()
                    .ok_or(std::io::ErrorKind::InvalidInput)?,
            )?
            .sync_all()?;
            Ok(bytes)
        })
        .await;
    let (bytes, storage_ok) = match finalized {
        Ok(Ok(n)) => (n, true),
        _ => {
            let files = services.clone();
            let id = job.id;
            let bytes = services
                .spawn_blocking(move || {
                    retained_media(&files, id, true).map_or(0, |(_, bytes)| bytes)
                })
                .await
                .unwrap_or(0);
            (bytes, false)
        }
    };
    let message = if !storage_ok {
        "Could not finalize the recording; partial media was retained"
    } else if bytes == 0 && outcome.is_ok() {
        "No media was recorded"
    } else {
        outcome.unwrap_or_else(|m| m)
    };
    let status = if bytes == 0 {
        Status::Failed
    } else if outcome.is_ok() && storage_ok {
        Status::Completed
    } else {
        Status::Partial
    };
    transition(services, job, status, bytes, message).await;
}

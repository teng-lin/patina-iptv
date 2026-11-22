//! App-open DVR. Durable intent is committed before acknowledgement; media workers
//! have no rendered player and use the same connection budget as viewing surfaces.
mod completion;
pub mod host;
#[cfg(test)]
mod recovery_tests;
mod rules;
mod shared;
#[cfg(test)]
mod shared_tests;
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
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(test)]
type PlaybackCompletion = (
    &'static str,
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);

// Captured before service/provider waits, checked again by the accepting writer.
#[derive(Clone)]
struct Admission {
    services: Services,
    recordings: Arc<State>,
    source: SourceId,
    incarnation: String,
    target: iptv_core::parental::Target,
    parental: crate::parental::Snapshot,
}
impl Admission {
    fn authorize(&self, store: &iptv_store_sqlite::Store) -> iptv_store_sqlite::Result<()> {
        if !self.services.is_current()
            || self.recordings.is_stopping()
            || store.source_incarnation(self.source)? != self.incarnation
            || !crate::parental::native_allowed(store, &self.target, &self.parental)?
        {
            return Err(iptv_store_sqlite::StoreError::Unavailable(
                "Recording access changed",
            ));
        }
        Ok(())
    }
}

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
    input: Option<Arc<shared::Incoming>>,
    pending: Option<u64>,
}
impl Lease {
    fn connection(&self) -> Option<u64> {
        self.input.as_ref().map(|input| input.id).or(self.pending)
    }
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
    scopes: HashMap<i64, (String, SourceConfig)>,
    viewer_revision: HashMap<usize, u64>,
    source_incarnations: HashMap<i64, String>,
    limits: HashMap<i64, u32>,
    provider_limits: HashMap<i64, u32>,
    viewers: HashMap<usize, Lease>,
    inputs: HashMap<u64, shared::Registered>,
    suspended_viewers: HashMap<usize, Lease>,
    active: HashMap<JobId, (Lease, CancelToken)>,
    jobs: Vec<Job>,
    finalization_errors: std::collections::HashSet<JobId>,
    unshareable: shared::Unshareable,
    wake_capability: iptv_core::recording::WakeCapability,
    wake_reported: bool,
    published_schedule: Option<(i64, bool)>,
    intent_revision: u64,
    recurring: bool,
    guide_check_at: i64,
    ready: bool,
    stopped: bool,
    completed_run: u64,
}
impl Data {
    fn schedule(&self, at: i64) -> (i64, bool) {
        let next = self
            .jobs
            .iter()
            .filter(|job| job.status == Status::Scheduled)
            .map(|job| job.request.start)
            .min()
            .unwrap_or(0);
        let next = if self.recurring {
            // Before the first guide poll, request an immediate wake rather
            // than postpone a series check by another thirty minutes.
            let guides = if self.guide_check_at > 0 {
                self.guide_check_at
            } else {
                at
            };
            if next > 0 { next.min(guides) } else { guides }
        } else {
            next
        };
        (
            next,
            !self.active.is_empty()
                || self.jobs.iter().any(|job| {
                    job.status == Status::Scheduled
                        && job.request.start <= at
                        && job.request.end > at
                }),
        )
    }
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
            input: None,
            pending: None,
        }
    }
    fn capacity(&self, source: i64, replacing: Option<usize>) -> bool {
        self.capacity_with(source, replacing, None)
    }
    fn capacity_with(&self, source: i64, replacing: Option<usize>, input: Option<u64>) -> bool {
        self.capacity_for(&self.lease(source), replacing, input)
    }
    fn capacity_for(&self, wanted: &Lease, replacing: Option<usize>, input: Option<u64>) -> bool {
        let source = wanted.source;
        let same = |lease: &Lease| same_account(&lease.account, &wanted.account);
        // A replacement may retire its sole old viewer owner before opening.
        // Other admissions still count that registry entry until closure joins.
        let superseded = replacing
            .and_then(|surface| self.viewers.get(&surface))
            .and_then(Lease::connection)
            .filter(|id| {
                !self.viewers.iter().any(|(surface, lease)| {
                    Some(*surface) != replacing && lease.connection() == Some(*id)
                }) && !self
                    .active
                    .values()
                    .any(|(lease, _)| lease.connection() == Some(*id))
                    && !self
                        .suspended_viewers
                        .values()
                        .any(|lease| lease.connection() == Some(*id))
            });
        // Existing leases retain their original account and limit when a playlist is edited.
        let leases = self
            .viewers
            .iter()
            .filter(|(surface, _)| Some(**surface) != replacing)
            .map(|(_, lease)| lease)
            .chain(self.active.values().map(|(lease, _)| lease))
            .chain(
                self.inputs
                    .values()
                    .filter(|registered| registered.lease.connection() != superseded)
                    .map(|input| &input.lease),
            )
            .chain(
                self.suspended_viewers
                    .values()
                    .filter(|lease| lease.input.is_some()),
            )
            .filter(|lease| same(lease));
        let mut inputs = vec![input];
        let mut limit = wanted
            .limit
            .min(self.limits.get(&source).copied().unwrap_or(1));
        let mut occupied = std::collections::HashSet::new();
        for lease in leases {
            occupied.insert(lease.source);
            inputs.push(lease.connection());
            limit = limit.min(lease.limit);
        }
        for (id, account) in &self.accounts {
            // Unknown M3U association matters while it owns a connection. An
            // idle playlist cannot establish an alias or lower another account's
            // allowance just by being present in the library.
            let known =
                !matches!(account, Account::Unknown) && !matches!(wanted.account, Account::Unknown);
            if same_account(account, &wanted.account)
                && (*id == source || occupied.contains(id) || known)
            {
                limit = limit
                    .min(self.lease(*id).limit)
                    .min(self.limits.get(id).copied().unwrap_or(1));
            }
        }
        iptv_core::live::upstream_count(inputs) <= limit as usize
    }
}
struct ContextEntry {
    pointer: usize,
    config: SourceConfig,
    value: iptv_guide::playback::PlaybackContext,
}
#[derive(Default)]
struct Execution {
    generation: u64,
    started: bool,
    shutdown: CancelToken,
}
/// Cancellation and serialization for one viewing surface's preparation.
type SurfacePreparation = (CancelToken, Arc<tokio::sync::Mutex<()>>);

#[derive(Default)]
pub(crate) struct State {
    host: Mutex<Option<Arc<crate::store_service::Service>>>,
    data: Mutex<Data>,
    changed: Condvar,
    publication: Mutex<()>,
    execution: Mutex<Execution>,
    contexts: Mutex<HashMap<i64, ContextEntry>>,
    inputs_changed: tokio::sync::Notify,
    // Connection IDs belong to this directory-owned registry, including retries.
    next_input: AtomicU64,
    preparations: Mutex<HashMap<usize, SurfacePreparation>>,
    #[cfg(test)]
    preparation_arrivals: Mutex<HashMap<usize, tokio::sync::oneshot::Sender<()>>>,
    media_ops: tokio::sync::Mutex<()>,
    #[cfg(test)]
    playback_completion: Mutex<Option<PlaybackCompletion>>,
    playback: Mutex<Option<(JobId, iptv_net::recording_file::RecordingFile)>>,
}
impl State {
    fn lock(&self) -> std::sync::MutexGuard<'_, Data> {
        self.data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn execution(&self) -> std::sync::MutexGuard<'_, Execution> {
        self.execution
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn is_stopping(&self) -> bool {
        let execution = self.execution();
        execution.started && execution.shutdown.is_canceled() && !self.lock().stopped
    }
    fn publish_schedule(&self, services: &Services, revision: u64, force: bool) -> bool {
        // The common poll must not wait for an unchanged, slow OS permission
        // refresh before it can start due captures.
        if !force {
            let data = self.lock();
            if data.intent_revision == revision
                && data.published_schedule == Some(data.schedule(now()))
            {
                return true;
            }
        }
        // UI acknowledgements and native polls publish in order. A late poll
        // cannot replace the alarm belonging to a newly accepted edit.
        let _publication = self
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !services.is_current() {
            return false;
        }
        let (generation, stopping) = {
            let execution = self.execution();
            (
                execution.generation,
                execution.started && execution.shutdown.is_canceled(),
            )
        };
        let (next, active) = {
            let data = self.lock();
            if data.intent_revision != revision {
                return false;
            }
            // A cold/restarted host first reconciles durable jobs and rules.
            // An early foreground callback must leave its saved alarm intact.
            // Its first successful poll will publish after loading the snapshot.
            if !data.ready || stopping {
                return true;
            }
            if data.recurring && data.guide_check_at <= now() {
                return true;
            }
            let schedule = data.schedule(now());
            if !force && data.published_schedule == Some(schedule) {
                return true;
            }
            schedule
        };
        let capability = (services.ports().recording_schedule)(next, active);
        let execution = self.execution();
        if !services.is_current() || execution.generation != generation {
            return false;
        }
        let mut data = self.lock();
        if data.intent_revision == revision {
            data.wake_capability = capability;
            data.wake_reported = true;
            data.published_schedule = Some((next, active));
        }
        true
    }
    pub(crate) fn stop(&self) -> bool {
        self.cancel_all_preparations();
        let generation = {
            let execution = self.execution();
            if !execution.started {
                return true;
            }
            execution.shutdown.cancel();
            execution.generation
        };
        // A replacement may already be running when this waiter wakes. Wait
        // for the execution we canceled, rather than its successor's flag.
        self.changed
            .wait_timeout_while(self.lock(), Duration::from_secs(15), |d| {
                d.completed_run < generation
            })
            .map(|(data, _)| data.completed_run >= generation)
            .unwrap_or(false)
    }
}

/// Media lives beside the catalog directory, so catalog restore cannot delete
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
        self.recordings.cancel_preparation(surface);
        let lease = data.lease(source.0);
        data.suspended_viewers.remove(&surface);
        *data.viewer_revision.entry(surface).or_default() += 1;
        data.viewers.insert(surface, lease);
        drop(data);
        self.retire_previous_viewing(surface);
        true
    }
    pub fn release_viewing(&self, surface: usize) {
        self.recordings.cancel_preparation(surface);
        if self.recordings_detached.get() {
            return;
        }
        let mut data = self.recordings.lock();
        *data.viewer_revision.entry(surface).or_default() += 1;
        data.viewers.remove(&surface);
        data.suspended_viewers.remove(&surface);
        drop(data);
        self.retire_previous_viewing(surface);
    }
    pub fn recording_snapshot(&self) -> Vec<Job> {
        self.recordings.lock().jobs.clone()
    }

    async fn recording_admission(&self, request: &Request) -> Result<Admission, Error> {
        let services = self.services();
        let source = SourceId(request.source);
        let incarnation = services
            .read(move |store| store.source_incarnation(source))
            .await?;
        let target = iptv_core::parental::Target::live(request.source, request.channel.as_str());
        self.parental_check(&target)
            .await
            .map_err(|_| Error::unavailable("Unlock this recording with the parental PIN"))?;
        let parental = self.parental_snapshot();
        Ok(Admission {
            services,
            recordings: self.recordings.clone(),
            source,
            incarnation,
            target,
            parental,
        })
    }

    pub async fn schedule_recording(&self, request: Request) -> Result<JobId, Error> {
        request.validate(now()).map_err(Error::unavailable)?;
        let admission = self.recording_admission(&request).await?;
        if self.recordings.is_stopping() || !self.services().is_current() {
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
        refresh_accounts(&self.services(), &self.recordings)
            .await
            .map_err(|_| Error::unavailable("Recording sources unavailable"))?;
        self.recordings.retire_unused().await;
        if request.start <= now() && !self.recordings.lock().recording_capacity(&request) {
            return Err(Error::unavailable(
                "No provider connection available; stop another stream or check Streams at once",
            ));
        }
        let mut random = [0u8; 8];
        (self.services().ports().entropy)(&mut random)
            .map_err(|_| Error::unavailable("Recording identity unavailable"))?;
        let id = JobId((i64::from_ne_bytes(random) & i64::MAX).max(1));
        let id = self
            .services()
            .call(move |store| {
                store.schedule_recording_checked(id, &request, now(), |store| {
                    admission.authorize(store)
                })
            })
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
            {
                let mut data = self.recordings.lock();
                data.intent_revision += 1;
                data.jobs.retain(|existing| existing.id != id);
                data.jobs.push(job);
            }
            self.refresh_recording_wake().await?;
        }
        Ok(())
    }
    /// Current OS registration, not a promise about content or provider capacity.
    pub fn recording_capabilities(&self) -> iptv_core::capabilities::Recording {
        let services = self.services();
        let background = services.ports().background_recordings;
        let data = self.recordings.lock();
        iptv_core::capabilities::Recording {
            background,
            wake: (services.is_current() && data.wake_reported && !data.stopped)
                .then_some(data.wake_capability),
        }
    }
    pub fn recording_wake_notice(&self) -> Option<&'static str> {
        let data = self.recordings.lock();
        let active = data.recurring || data.jobs.iter().any(|job| job.status.active());
        drop(data);
        active
            .then(|| {
                self.recording_capabilities()
                    .wake
                    .and_then(|wake| wake.notice())
            })
            .flatten()
    }
    pub async fn refresh_recording_wake(&self) -> Result<(), Error> {
        self.refresh_recording_wake_with_notice(true).await
    }
    /// Recheck permission on foreground return; the recording screen keeps the
    /// current explanation visible without repeating a toast on every return.
    pub async fn resume_recording_wake(&self) -> Result<(), Error> {
        self.refresh_recording_wake_with_notice(false).await
    }
    async fn refresh_recording_wake_with_notice(&self, notify: bool) -> Result<(), Error> {
        let services = self.services();
        loop {
            if !services.is_current() {
                return Err(Error::unavailable("Recording service unavailable"));
            }
            let revision = self.recordings.lock().intent_revision;
            let state = self.recordings.clone();
            let work = services.clone();
            let published = services
                .spawn_blocking(move || state.publish_schedule(&work, revision, true))
                .await
                .map_err(|_| Error::unavailable("Recording service unavailable"))?;
            if published && self.recordings.lock().intent_revision == revision {
                break;
            }
        }
        if notify && let Some(notice) = self.recording_wake_notice() {
            self.emit(crate::RuntimeEvent::Notify(notice));
        }
        Ok(())
    }
    /// Cancels a future job, or requests that an active recording stop and retain media.
    pub async fn update_recording(&self, id: JobId, request: Request) -> Result<(), Error> {
        let admission = self.recording_admission(&request).await?;
        if !self
            .services()
            .call(move |store| {
                store.update_recording_checked(id, &request, now(), |store| {
                    admission.authorize(store)
                })
            })
            .await?
        {
            return Err(Error::unavailable(
                "This recording has already started or was canceled",
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
            && job.status == Status::Scheduled
            && job.bytes > 0
        {
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
                if matches!(
                    recover_media(&self.services(), &stopped).await,
                    completion::Transition::Failed
                ) {
                    return Err(Error::unavailable(
                        "Recording media needs recovery; check storage",
                    ));
                }
                return Ok(());
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
                    Status::Canceled,
                    0,
                    "Canceled",
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
        for job in &mut jobs {
            if job.status.active() && self.recordings.lock().finalization_errors.contains(&job.id) {
                job.message =
                    "Recording state could not be saved; check storage and restart to recover"
                        .into();
            }
        }
        Ok(jobs)
    }
    #[cfg(test)]
    async fn recording_playback_checkpoint(&self, stage: &'static str) {
        let held = {
            let mut completion = self.recordings.playback_completion.lock().unwrap();
            if completion
                .as_ref()
                .is_some_and(|(expected, _, _)| *expected == stage)
            {
                completion.take()
            } else {
                None
            }
        };
        if let Some((_, arrived, release)) = held {
            let _ = arrived.send(());
            let _ = release.await;
        }
    }
    pub async fn open_recording(&self, id: JobId) -> Result<PlaySource, Error> {
        let _operation = self.recordings.media_ops.lock().await;
        let (file, target) = self.recording_file_with_target(id).await?;
        let relay = self
            .services()
            .spawn(async move { iptv_net::recording_file::RecordingFile::start(file) })
            .await
            .map_err(|_| Error::unavailable("Recording unavailable"))?
            .map_err(|_| Error::unavailable("Recording unavailable"))?;
        #[cfg(test)]
        self.recording_playback_checkpoint("relay").await;
        // A filesystem/relay await must not extend the PIN grant or old owner.
        // Keep the unaccepted relay local so rejection closes its capability.
        self.require_recording_playback(&target)?;
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
        self.recording_file_with_target(id)
            .await
            .map(|(file, _)| file)
    }
    fn require_recording_playback(
        &self,
        target: &iptv_core::parental::Target,
    ) -> Result<(), Error> {
        if !self.services().is_current() || !self.parental_allowed(target) {
            return Err(Error::unavailable(
                "Unlock this recording with the parental PIN",
            ));
        }
        Ok(())
    }
    async fn recording_file_with_target(
        &self,
        id: JobId,
    ) -> Result<(PathBuf, iptv_core::parental::Target), Error> {
        let job = self
            .services()
            .read(move |store| store.recording_job(id))
            .await?
            .ok_or_else(|| Error::unavailable("Recording unavailable"))?;
        let target =
            iptv_core::parental::Target::live(job.request.source, job.request.channel.as_str());
        self.parental_check(&target)
            .await
            .map_err(|_| Error::unavailable("Unlock this recording with the parental PIN"))?;
        if !matches!(job.status, Status::Completed | Status::Partial) || job.bytes == 0 {
            return Err(Error::unavailable("This recording has no playable media"));
        }
        let services = self.services();
        let file = self
            .services()
            .spawn_blocking(move || retained_media(&services, id, job.status == Status::Partial))
            .await
            .ok()
            .flatten()
            .map(|(file, _)| file)
            .ok_or_else(|| Error::unavailable("The recording file is missing"))?;
        #[cfg(test)]
        self.recording_playback_checkpoint("file").await;
        self.require_recording_playback(&target)?;
        Ok((file, target))
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
                .map(|source| {
                    let Some(stored) = store.source(source.id)? else {
                        return Ok(None);
                    };
                    let incarnation = store.source_incarnation(source.id)?;
                    Ok(Some((stored, incarnation)))
                })
                .collect::<iptv_store_sqlite::Result<Vec<_>>>()
        })
        .await?;
    let settings = services.settings();
    let mut data = state.lock();
    let previous_accounts = std::mem::take(&mut data.accounts);
    let previous_incarnations = std::mem::take(&mut data.source_incarnations);
    data.limits.clear();
    data.scopes.clear();
    for (source, incarnation) in sources.into_iter().flatten() {
        let account = account(&source.source.config);
        if previous_incarnations
            .get(&source.id.0)
            .is_some_and(|old| old != &incarnation)
            || previous_accounts
                .get(&source.id.0)
                .is_some_and(|old| old != &account)
        {
            data.provider_limits.remove(&source.id.0);
        }
        let limit = connection_limit(
            &settings,
            source.id.0,
            data.provider_limits.get(&source.id.0).copied(),
        );
        data.scopes.insert(
            source.id.0,
            (incarnation.clone(), source.source.config.clone()),
        );
        data.accounts.insert(source.id.0, account);
        data.source_incarnations.insert(source.id.0, incarnation);
        data.limits.insert(source.id.0, limit);
    }
    let current = data
        .accounts
        .keys()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    data.provider_limits
        .retain(|source, _| current.contains(source));
    Ok(())
}

fn connection_limit(settings: &iptv_settings::Settings, source: i64, observed: Option<u32>) -> u32 {
    const KEY: &str = "playlists.max_connections";
    let target = iptv_settings::Target::playlist(source.to_string());
    if settings.source(KEY, &target).ok().flatten().is_some()
        && let Ok(iptv_settings::Value::Int(limit)) = settings.get_in(KEY, &target)
    {
        return limit.clamp(1, 20) as u32;
    }
    // The conservative schema default is for unknown providers. It must not
    // override an allowance actually reported by an authenticated provider.
    observed.filter(|limit| *limit > 0).unwrap_or(1)
}
async fn transition(
    services: &Services,
    job: &Job,
    to: Status,
    bytes: u64,
    message: &'static str,
) -> completion::Transition {
    let (id, from) = (job.id, job.status);
    completion::retry(services, move |store| {
        store.transition_recording(id, from, to, bytes, message)
    })
    .await
}

async fn recover_media(services: &Services, job: &Job) -> completion::Transition {
    if let Some(result) = completion::recover(services, job.id).await {
        return result;
    }
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
        return completion::Transition::Applied;
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
    .await
}
async fn finish_unstarted(
    services: &Services,
    job: &Job,
    status: Status,
    message: &'static str,
) -> completion::Transition {
    if job.bytes > 0 {
        recover_media(services, job).await
    } else {
        transition(services, job, status, 0, message).await
    }
}

async fn recover(services: &Services) -> Result<(), crate::model::StoreError> {
    for job in services.read(|store| store.recording_jobs()).await? {
        if completion::recover(services, job.id).await.is_some() {
            continue;
        }
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
            finish_unstarted(
                services,
                &job,
                Status::Missed,
                "The recording window elapsed while the host was unavailable",
            )
            .await;
        }
    }
    Ok(())
}
async fn run(services: Services, state: Arc<State>, shutdown: CancelToken) {
    state.lock().finalization_errors.clear();
    let mut retry = Duration::from_millis(250);
    loop {
        if !services.is_current() || shutdown.is_canceled() {
            return;
        }
        if recover(&services).await.is_ok() && refresh_accounts(&services, &state).await.is_ok() {
            break;
        }
        tokio::select! {
            _ = shutdown.canceled() => return,
            _ = tokio::time::sleep(retry) => {},
        }
        retry = (retry * 2).min(Duration::from_secs(5));
    }
    let mut tasks = tokio::task::JoinSet::new();
    let mut rules_at = 0;
    loop {
        while tasks.try_join_next().is_some() {}
        state.retire_unused().await;
        if !services.is_current() || shutdown.is_canceled() {
            shutdown.cancel();
            while tasks.join_next().await.is_some() {}
            state.retire_unused().await;
            break;
        }
        if refresh_accounts(&services, &state).await.is_err() {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }
        let revision = state.lock().intent_revision;
        let Ok(jobs) = services.read(|store| store.recording_jobs()).await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        if now() >= state.lock().guide_check_at {
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
                    let cancel = shutdown.clone();
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
            state.lock().guide_check_at = now().saturating_add(30 * 60);
        }
        if now() >= rules_at {
            let Ok(rules) = services.read(|store| store.recording_rules()).await else {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            };
            {
                let mut data = state.lock();
                if data.intent_revision == revision {
                    data.recurring = rules.iter().any(|rule| rule.enabled);
                }
            }
            let _ = rules::materialize(&services).await;
            rules_at = now().saturating_add(30);
        }
        {
            let mut data = state.lock();
            if data.intent_revision != revision {
                continue;
            }
            data.jobs = jobs.clone();
            data.ready = true;
        }
        if !state.publish_schedule(&services, revision, false) {
            continue;
        }
        #[cfg(test)]
        state.changed.notify_all();
        for mut job in jobs {
            if state.lock().finalization_errors.contains(&job.id) {
                continue;
            }
            if job.status == Status::Recording && !state.lock().active.contains_key(&job.id) {
                if !state.lock().finalization_errors.contains(&job.id)
                    && matches!(
                        recover_media(&services, &job).await,
                        completion::Transition::Failed
                    )
                {
                    state.lock().finalization_errors.insert(job.id);
                }
                continue;
            }
            if job.status != Status::Scheduled || job.request.start > now() {
                continue;
            }
            if job.request.end <= now() {
                if matches!(
                    finish_unstarted(
                        &services,
                        &job,
                        Status::Missed,
                        "The scheduled recording window was missed"
                    )
                    .await,
                    completion::Transition::Failed
                ) {
                    state.lock().finalization_errors.insert(job.id);
                }
                continue;
            }
            let stop = CancelToken::new();
            let reserved = {
                let mut data = state.lock();
                if data.active.len() >= 4 || !data.recording_capacity(&job.request) {
                    false
                } else {
                    let mut lease = data.lease(job.request.source);
                    lease.input = data.shared_for(job.request.source, &job.request.channel);
                    data.active.insert(job.id, (lease, stop.clone()));
                    true
                }
            };
            if !reserved {
                if matches!(
                    finish_unstarted(
                        &services,
                        &job,
                        Status::Failed,
                        "No provider connection available; check Streams at once"
                    )
                    .await,
                    completion::Transition::Failed
                ) {
                    state.lock().finalization_errors.insert(job.id);
                }
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
            let shutdown = shutdown.clone();
            tasks.spawn(async move {
                struct Release(Arc<State>, JobId);
                impl Drop for Release {
                    fn drop(&mut self) {
                        self.0.lock().active.remove(&self.1);
                    }
                }
                let _release = Release(state.clone(), job.id);
                record(&services, &state, &job, &stop, &shutdown).await;
            });
        }
        tokio::select! {
            _ = shutdown.canceled() => {},
            _ = tokio::time::sleep(Duration::from_secs(1)) => {},
        }
    }
}

struct Resolved {
    source: PlaySource,
    identity: iptv_core::live::Identity,
    config: SourceConfig,
}

async fn resolve(
    services: &Services,
    state: &State,
    job: &Job,
    stop: &CancelToken,
) -> Result<Resolved, &'static str> {
    let (source, channel) = (
        SourceId(job.request.source),
        ContentId::from_stored(job.request.channel.clone()),
    );
    let id = job.id;
    let (source, channel, incarnation) = services
        .read(move |store| {
            if !store.source_policy(source)?.enabled {
                return Err(iptv_store_sqlite::StoreError::Unavailable(
                    "Playlist disabled",
                ));
            }
            let result = (
                store.source(source)?,
                store.channel(source, &channel)?,
                store.source_incarnation(source)?,
            );
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
    // This job was durably admitted (and conditionally claimed) before capture.
    // A viewing relock cannot revoke accepted recording intent. Source/channel
    // availability and original incarnation still gate every provider resolution.
    let channel = channel.ok_or("Channel unavailable")?.channel;
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
        let client = services
            .network_client()
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
        crate::playback::clock(now(), None),
    );
    let mut stream = match resolution {
        iptv_guide::playback::Resolution::Play(source) => source,
        iptv_guide::playback::Resolution::StalkerLink(link) => {
            let iptv_guide::playback::StalkerLink {
                session: mut portal,
                cmd,
                channel_headers,
            } = *link;
            let client = services
                .network_client()
                .map_err(|_| "Network unavailable")?;
            let url = iptv_import::stalker_link(&client, &mut portal, &cmd, false, stop)
                .await
                .map_err(|_| "Portal stream unavailable")?;
            iptv_guide::playback::StalkerLink::source(&portal, url, &channel_headers)
        }
        _ => return Err("Refresh the playlist before recording this channel"),
    };
    let config = crate::settings::network_config(&services.settings());
    stream.default_header("User-Agent", &config.user_agent);
    let config = source.source.config;
    let expected_config = config.clone();
    let expected_incarnation = incarnation.clone();
    let expected_channel = channel.clone();
    let source_id = SourceId(job.request.source);
    let current = services
        .read(move |store| {
            Ok(store.recording_source_is_current(id)?
                && store.source_policy(source_id)?.enabled
                && store.source_incarnation(source_id)? == expected_incarnation
                && store
                    .source(source_id)?
                    .is_some_and(|source| source.source.config == expected_config)
                && store
                    .channel(source_id, &expected_channel.id)?
                    .is_some_and(|current| current.channel == expected_channel))
        })
        .await
        .map_err(|_| "Playlist unavailable")?;
    if !current {
        return Err("Playlist unavailable");
    }
    Ok(Resolved {
        source: stream,
        identity: iptv_core::live::Identity {
            source: source_id.0,
            incarnation,
            channel: job.request.channel.clone(),
        },
        config,
    })
}

async fn record(
    services: &Services,
    state: &State,
    job: &Job,
    stop: &CancelToken,
    shutdown: &CancelToken,
) {
    let partial = path(services, job.id, true);
    let directory = media_directory(services.directory());
    let exclude = services.ports().exclude_from_backup;
    let active: Vec<_> = state.lock().active.keys().copied().collect();
    let prepared = services
        .spawn_blocking(move || -> std::io::Result<()> {
            crate::storage::protect_directory(&directory, exclude)?;
            storage_admission(&directory, &active)?;
            // Advisory only: another process may consume space after this check.
            // ENOSPC during writes still flows through acknowledged finalization.
            if available_space(&directory).is_some_and(|bytes| bytes < 256 * 1024) {
                return Err(std::io::ErrorKind::StorageFull.into());
            }
            Ok(())
        })
        .await;
    let mut file = if matches!(prepared, Ok(Ok(()))) {
        iptv_net::recording::CaptureFile::open(&partial, MAX_BYTES, true)
            .await
            .ok()
    } else {
        None
    };
    let had_gap = Arc::new(AtomicBool::new(
        job.request.start < now().saturating_sub(5) || job.bytes > 0,
    ));
    let gap = had_gap.clone();
    let result = async {
        let Some(file) = file.as_mut() else {
            return Err("Recording storage unavailable");
        };
        let mut retry = Duration::from_secs(1);
        let mut capture = iptv_net::recording::CaptureState::default();
        loop {
            let resolved = match resolve(services, state, job, stop).await {
                Ok(source) => source,
                Err(_) => {
                    state.detach_capture(job.id);
                    state.retire_unused().await;
                    gap.store(true, Ordering::SeqCst);
                    tokio::time::sleep(retry).await;
                    retry = (retry * 2).min(Duration::from_secs(15));
                    continue;
                }
            };
            let source = resolved.source;
            let net = crate::settings::net_config(&services.settings());
            let policy = PlaybackPolicy {
                public_only: source.public_only,
                extra_roots_der: net.extra_roots_der,
                #[cfg(debug_assertions)]
                allow_private_fixture: net.allow_private_fixture,
            };
            let shared = {
                let mut data = state.lock();
                let matching = data
                    .shared_resolved(&resolved.identity, &resolved.config, &source, &policy)
                    .or_else(|| {
                        // Preserve partial/failure if the retained shared owner
                        // ended and no live owner can replace it; playback keeps
                        // its normal retry slot.
                        data.active
                            .get(&job.id)
                            .and_then(|(lease, _)| lease.input.clone())
                            .filter(|incoming| {
                                incoming.identity == resolved.identity
                                    && incoming.config == resolved.config
                                    && incoming.source == source
                                    && incoming.policy == policy
                                    && incoming.input.shared().is_some()
                                    && !incoming.input.is_running()
                            })
                    });
                // Reservation may have retained a different authentication or
                // trust context before resolution. Search every eligible owner
                // before asking for another provider connection.
                if let Some(incoming) = &matching
                    && let Some((lease, _)) = data.active.get_mut(&job.id)
                {
                    lease.input = Some(incoming.clone());
                }
                matching
            };
            let outcome = if let Some(incoming) = shared {
                if !incoming.input.is_running() {
                    return Err(iptv_net::recording::Failure::Network.message());
                }
                // A shared input ending before stop/deadline retains partial
                // media and frees capture's lease for playback's normal retry.
                // An independent capture retry must not strand that viewer.
                return file
                    .capture_shared(incoming.input.shared().expect("shared capture lease"))
                    .await
                    .map_err(|error| error.message());
            } else {
                // A reauthentication/link change or failed incoming stream is
                // never allowed to inherit its sharing discount. A fresh
                // independent request needs its own permitted connection.
                state.detach_capture(job.id);
                state.retire_unused().await;
                {
                    let mut data = state.lock();
                    if let Some((lease, token)) = data.active.remove(&job.id) {
                        let allowed = data.capacity(job.request.source, None);
                        data.active.insert(job.id, (lease, token));
                        if !allowed {
                            return Err("No provider connection available; check Streams at once");
                        }
                    }
                }
                file.capture_stream(&source.url, &source.headers, policy, &mut capture)
                    .await
            };
            match outcome {
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
        _ = shutdown.canceled() => Err("Interrupted when the app closed"),
        _ = stop.canceled() => Ok("Stopped by you"),
        _ = async {
            let limit = tokio::time::Instant::now() + Duration::from_secs(remaining);
            while now() < job.request.end && tokio::time::Instant::now() < limit {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        } => Ok("Recording finished"),
        result = result => result.map(|_| "Recording finished"),
    };
    // The canceled receive future no longer owns file writes. Keep the active
    // reservation until its worker acknowledges drain, sync and descriptor close.
    let outcome = match file {
        Some(file) => {
            if file.finish().await.is_ok() {
                outcome
            } else {
                Err("Recording storage unavailable")
            }
        }
        None => Err("Recording storage unavailable"),
    };
    if shutdown.is_canceled() && state.background() && job.request.end > now() {
        let bytes = std::fs::symlink_metadata(&partial).map_or(0, |m| m.len() / 188 * 188);
        if matches!(
            transition(
                services,
                job,
                Status::Scheduled,
                bytes,
                "Resuming interrupted recording"
            )
            .await,
            completion::Transition::Failed
        ) {
            state.lock().finalization_errors.insert(job.id);
        }
        return;
    }
    let outcome = if outcome.is_ok() && had_gap.load(Ordering::SeqCst) {
        Err("Recording retained with a gap after interruption")
    } else {
        outcome
    };
    if matches!(
        completion::finish(services, job, outcome).await,
        completion::Transition::Failed
    ) {
        state.lock().finalization_errors.insert(job.id);
    }
}

/// Retained bytes plus the unfilled part of every active reservation. Active
/// spool bytes are already on disk and must not be counted twice.
fn storage_admission(directory: &Path, active: &[JobId]) -> std::io::Result<()> {
    let mut total = 0u64;
    let mut remaining = active
        .iter()
        .map(|id| (*id, MAX_BYTES))
        .collect::<HashMap<_, _>>();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let metadata = entry.path().symlink_metadata()?;
        total = total.saturating_add(metadata.len());
        if let Some(id) = entry
            .path()
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<i64>().ok())
            && entry
                .path()
                .extension()
                .is_some_and(|s| s == "part" || s == "ts")
            && let Some(reserved) = remaining.get_mut(&JobId(id))
        {
            *reserved = reserved.saturating_sub(metadata.len());
        }
    }
    total = remaining
        .values()
        .fold(total, |total, bytes| total.saturating_add(*bytes));
    if total > 32 * 1024 * 1024 * 1024 {
        return Err(std::io::ErrorKind::StorageFull.into());
    }
    Ok(())
}

/// Native hosts with statvfs can reject already-exhausted storage. Other hosts
/// rely on the write result; inability to query capacity is not an empty disk.
#[cfg(unix)]
fn available_space(directory: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(directory.as_os_str().as_bytes()).ok()?;
    let mut space = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: path is NUL-terminated; the out pointer is valid and read only when
    // statvfs reports success, which initializes the entire structure.
    let status = unsafe { libc::statvfs(path.as_ptr(), space.as_mut_ptr()) };
    if status != 0 {
        return None;
    }
    // SAFETY: successful statvfs initialized space above.
    let space = unsafe { space.assume_init() };
    Some(
        (u128::from(space.f_bavail).saturating_mul(u128::from(space.f_frsize)))
            .min(u128::from(u64::MAX)) as u64,
    )
}
#[cfg(not(unix))]
fn available_space(_: &Path) -> Option<u64> {
    None
}

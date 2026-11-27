//! Owner-scoped release discovery and durable scheduling. No view owns this state.
//!
//! Times are UTC Unix seconds supplied by the caller. Monthly means one calendar
//! month after the last successful check (clamped to the final day of that month).
//! Checking never authorizes downloading or installing a package.

use std::{
    io,
    path::Path,
    sync::{Arc, Mutex},
};

use chrono::{DateTime, Months, Utc};

use super::github::{CheckError, ReleaseCheck, ReleaseStatus};

pub use crate::update_model::{Frequency, Snapshot, Trigger};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckToken(u64);

#[derive(Clone, Debug, Default)]
struct Durable {
    last_success: Option<i64>,
    last_attempt: Option<i64>,
    retry_after: Option<i64>,
    failures: u32,
    skipped_version: Option<String>,
    notified_version: Option<String>,
    /// A release discovered while its notice could not yet be acknowledged.
    /// Persist only its identity; refresh GitHub metadata after a restart.
    pending_version: Option<String>,
}

#[derive(Default)]
pub struct Controller {
    durable: Durable,
    cached: Option<ReleaseCheck>,
    error: Option<CheckError>,
    notice: Option<ReleaseCheck>,
    active: Option<CheckToken>,
    next_token: u64,
    startup_attempted: bool,
    startup_pending: bool,
    manual_requested: bool,
    revision: u64,
    writes: Arc<Mutex<u64>>,
}

impl Controller {
    /// Preserve explicit intent while the host awaits restored state or metadata.
    pub fn queue_manual_check(&mut self) {
        self.manual_requested = true;
    }

    pub fn manual_check_pending(&self) -> bool {
        self.manual_requested
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            checking: self.active.is_some(),
            cached: self.cached.clone(),
            error: self.error.clone(),
            notice: self.notice.clone(),
            last_success: self.durable.last_success,
            last_attempt: self.durable.last_attempt,
            retry_after: self.durable.retry_after,
            skipped_version: self.durable.skipped_version.clone(),
        }
    }

    pub fn begin_check(
        &mut self,
        now: i64,
        trigger: Trigger,
        frequency: Frequency,
    ) -> Option<CheckToken> {
        if trigger == Trigger::Startup && frequency == Frequency::OnLaunch {
            self.startup_pending = !self.startup_attempted;
        }
        let trigger = if self.manual_requested {
            Trigger::Manual
        } else {
            trigger
        };
        if self.active.is_some() {
            // Joining a fetch consumes the request, but does not acknowledge
            // its result: the panel may close before the response arrives.
            if trigger == Trigger::Manual {
                self.manual_requested = false;
            }
            if self.startup_pending {
                self.startup_attempted = true;
                self.startup_pending = false;
            }
            return None;
        }
        if trigger != Trigger::Manual {
            if frequency == Frequency::Never
                || (self.durable.retry_after.is_some_and(|retry| now < retry)
                    && self.durable.last_attempt.is_none_or(|last| now >= last))
            {
                return None;
            }
            let pending = self.cached.is_none()
                && self
                    .durable
                    .pending_version
                    .as_ref()
                    .is_some_and(|version| {
                        self.durable.skipped_version.as_ref() != Some(version)
                            && self.durable.notified_version.as_ref() != Some(version)
                    });
            let due = pending
                || match frequency {
                    Frequency::Never => false,
                    Frequency::OnLaunch => self.startup_pending && !self.startup_attempted,
                    Frequency::Daily | Frequency::Weekly | Frequency::Monthly => {
                        self.durable.last_success.is_none_or(|last| {
                            // A clock correction must not suppress checks for months.
                            now < last || now >= next_due(last, frequency)
                        })
                    }
                };
            if !due {
                return None;
            }
        }
        self.next_token = self.next_token.wrapping_add(1);
        let token = CheckToken(self.next_token);
        self.active = Some(token);
        self.startup_attempted |= trigger == Trigger::Startup || self.startup_pending;
        self.startup_pending = false;
        if trigger == Trigger::Manual {
            self.manual_requested = false;
        }
        self.error = None;
        self.durable.last_attempt = Some(now);
        self.revision += 1;
        Some(token)
    }

    /// Returns false for a completion from an operation this owner no longer owns.
    pub fn finish_check(
        &mut self,
        token: CheckToken,
        now: i64,
        result: Result<ReleaseCheck, CheckError>,
    ) -> bool {
        let Some(active) = self.active else {
            return false;
        };
        if active != token {
            return false;
        }
        self.active = None;
        match result {
            Ok(check) => {
                self.durable.last_success = Some(now);
                self.durable.retry_after = None;
                self.durable.failures = 0;
                self.error = None;
                let newer = matches!(
                    check.status,
                    ReleaseStatus::UpdateAvailable | ReleaseStatus::NoCompatiblePackage
                );
                if newer
                    && self.durable.skipped_version.as_deref() != Some(&check.release.version)
                    && self.durable.notified_version.as_deref() != Some(&check.release.version)
                {
                    self.durable.pending_version = Some(check.release.version.clone());
                    self.notice = Some(check.clone());
                } else {
                    self.durable.pending_version = None;
                    self.notice = None;
                }
                self.cached = Some(check);
            }
            Err(error) => {
                self.error = Some(error);
                self.durable.failures = self.durable.failures.saturating_add(1);
                let delay = match self.durable.failures {
                    1 => 15 * 60,
                    2 => 60 * 60,
                    3 => 6 * 60 * 60,
                    _ => 24 * 60 * 60,
                };
                self.durable.retry_after = Some(now.saturating_add(delay));
                // Retain the last successful release and any unacknowledged notice.
            }
        }
        self.revision += 1;
        true
    }

    pub fn skip_version(&mut self, version: &str) {
        self.durable.skipped_version = Some(version.to_owned());
        if self.durable.pending_version.as_deref() == Some(version) {
            self.durable.pending_version = None;
        }
        self.acknowledge_notice();
    }

    /// Called after the notice is shown or dismissed, not merely after fetching.
    pub fn acknowledge_notice(&mut self) {
        if let Some(check) = self.notice.take() {
            self.durable.notified_version = Some(check.release.version);
            self.durable.pending_version = None;
        }
        self.revision += 1;
    }

    fn restore(&mut self, value: serde_json::Value) -> io::Result<()> {
        if value["version"].as_u64() != Some(1) || !value.is_object() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid update schedule",
            ));
        }
        let string = |name| {
            value[name]
                .as_str()
                .filter(|s| s.len() <= 128)
                .map(str::to_owned)
        };
        self.durable = Durable {
            last_success: value["last_success"].as_i64(),
            last_attempt: value["last_attempt"].as_i64(),
            retry_after: value["retry_after"].as_i64(),
            failures: value["failures"].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32,
            skipped_version: string("skipped_version"),
            notified_version: string("notified_version"),
            pending_version: string("pending_version"),
        };
        Ok(())
    }

    fn document(&self) -> serde_json::Value {
        serde_json::json!({
            "version": 1,
            "last_success": self.durable.last_success,
            "last_attempt": self.durable.last_attempt,
            "retry_after": self.durable.retry_after,
            "failures": self.durable.failures,
            "skipped_version": self.durable.skipped_version,
            "notified_version": self.durable.notified_version,
            "pending_version": self.durable.pending_version,
        })
    }
}

fn next_due(last: i64, frequency: Frequency) -> i64 {
    match frequency {
        Frequency::Daily => last.saturating_add(86_400),
        Frequency::Weekly => last.saturating_add(7 * 86_400),
        Frequency::Monthly => DateTime::<Utc>::from_timestamp(last, 0)
            .and_then(|time| time.checked_add_months(Months::new(1)))
            .map_or(i64::MAX, |time| time.timestamp()),
        Frequency::Never | Frequency::OnLaunch => i64::MAX,
    }
}

fn path(directory: &Path) -> std::path::PathBuf {
    // Profile restores swap the catalog directory; update preferences survive it.
    crate::backup::sibling(directory, "app-update").join("schedule.json")
}

fn persist_document(
    file: &Path,
    document: &serde_json::Value,
    revision: u64,
    writes: &Mutex<u64>,
    sync_directory: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    let mut written = writes
        .lock()
        .map_err(|_| io::Error::other("update schedule lock poisoned"))?;
    if revision < *written {
        return Ok(());
    }
    let directory = file
        .parent()
        .ok_or_else(|| io::Error::other("missing update schedule directory"))?;
    std::fs::create_dir_all(directory)?;
    let mut temp = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(&mut temp, document)?;
    temp.as_file().sync_all()?;
    temp.persist(file).map_err(|error| error.error)?;
    // The replacement is already visible even if syncing its directory fails.
    // Block older saves now; equal revisions may retry the durability step.
    *written = revision;
    sync_directory(directory)
}

impl crate::Runtime {
    pub fn app_update_snapshot(&self) -> Snapshot {
        self.lifecycle.app_updates.controller.borrow().snapshot()
    }

    pub fn queue_manual_app_update_check(&self) {
        self.lifecycle
            .app_updates
            .controller
            .borrow_mut()
            .queue_manual_check();
    }

    pub fn app_update_manual_check_pending(&self) -> bool {
        self.lifecycle
            .app_updates
            .controller
            .borrow()
            .manual_check_pending()
    }

    pub fn begin_app_update_check(
        &self,
        now: i64,
        trigger: Trigger,
        frequency: Frequency,
    ) -> Option<CheckToken> {
        self.lifecycle
            .app_updates
            .controller
            .borrow_mut()
            .begin_check(now, trigger, frequency)
    }

    pub fn finish_app_update_check(
        &self,
        token: CheckToken,
        now: i64,
        result: Result<ReleaseCheck, CheckError>,
    ) -> bool {
        self.lifecycle
            .app_updates
            .controller
            .borrow_mut()
            .finish_check(token, now, result)
    }

    pub fn skip_app_update_version(&self, version: &str) {
        self.lifecycle
            .app_updates
            .controller
            .borrow_mut()
            .skip_version(version);
    }

    pub fn acknowledge_app_update_notice(&self) {
        self.lifecycle
            .app_updates
            .controller
            .borrow_mut()
            .acknowledge_notice();
    }

    /// Call once, before starting scheduled or manual discovery. Errors leave the
    /// default schedule usable and must be reported by the host.
    pub async fn restore_app_update_state(&self) -> io::Result<()> {
        let file = path(self.data_dir());
        let document = self
            .services()
            .spawn(async move {
                tokio::task::spawn_blocking(move || {
                    use io::Read;
                    let file = match std::fs::File::open(file) {
                        Ok(file) => file,
                        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                        Err(error) => return Err(error),
                    };
                    let mut bytes = Vec::new();
                    file.take(16 * 1024 + 1).read_to_end(&mut bytes)?;
                    if bytes.len() > 16 * 1024 {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "update schedule too large",
                        ));
                    }
                    serde_json::from_slice(&bytes)
                        .map(Some)
                        .map_err(io::Error::from)
                })
                .await
                .map_err(io::Error::other)?
            })
            .await
            .map_err(io::Error::other)??;
        if let Some(document) = document {
            self.lifecycle
                .app_updates
                .controller
                .borrow_mut()
                .restore(document)?;
        }
        Ok(())
    }

    /// Persist after completing a check, skipping a version or acknowledging a
    /// notice. Ordered revisions prevent older async saves overwriting newer ones.
    pub async fn persist_app_update_state(&self) -> io::Result<()> {
        let file = path(self.data_dir());
        let (document, revision, writes) = {
            let state = self.lifecycle.app_updates.controller.borrow();
            (state.document(), state.revision, Arc::clone(&state.writes))
        };
        self.services()
            .spawn(async move {
                tokio::task::spawn_blocking(move || {
                    persist_document(&file, &document, revision, &writes, |_directory| {
                        #[cfg(unix)]
                        std::fs::File::open(_directory)?.sync_all()?;
                        Ok(())
                    })
                })
                .await
                .map_err(io::Error::other)?
            })
            .await
            .map_err(io::Error::other)?
    }
}

#[cfg(test)]
mod tests;

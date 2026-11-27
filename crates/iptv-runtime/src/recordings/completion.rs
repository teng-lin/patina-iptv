//! Durable terminal intent precedes rename; verified media precedes catalog acknowledgement.
use super::*;
use crate::model::RecordingCompletion;
use sha2::{Digest, Sha256};
use std::io::Read;

#[derive(Debug)]
pub(super) enum Transition {
    Applied,
    Superseded,
    Failed,
}

#[derive(Clone)]
pub(super) enum RecordingOperation {
    Retain(RecordingCompletion),
    Commit(RecordingCompletion),
    Prepare(Job, RecordingCompletion),
    Transition {
        id: iptv_core::recording::JobId,
        from: Status,
        to: Status,
        bytes: u64,
        message: &'static str,
    },
}

pub(super) async fn retry(services: &Services, operation: RecordingOperation) -> Transition {
    retry_with(|| {
        let services = services.clone();
        let operation = operation.clone();
        async move {
            match operation {
                RecordingOperation::Retain(completion) => {
                    services
                        .recordings()
                        .retain_partial_completion(completion)
                        .await
                }
                RecordingOperation::Commit(completion) => {
                    services.recordings().commit_completion(completion).await
                }
                RecordingOperation::Prepare(job, completion) => {
                    services
                        .recordings()
                        .prepare_completion(job, completion)
                        .await
                }
                RecordingOperation::Transition {
                    id,
                    from,
                    to,
                    bytes,
                    message,
                } => {
                    services
                        .recordings()
                        .transition(id, from, to, bytes, message)
                        .await
                }
            }
        }
    })
    .await
}

async fn retry_with<F, Fut>(mut operation: F) -> Transition
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<bool, crate::error::AdapterError>>,
{
    for attempt in 0..3 {
        let result = operation().await;
        match result {
            Ok(true) => return Transition::Applied,
            Ok(false) => return Transition::Superseded,
            Err(error) if error.retryable_write() && attempt < 2 => {
                tokio::time::sleep(Duration::from_millis(50 << attempt)).await;
            }
            Err(_) => {
                // No paths, media content, provider values or database diagnostics.
                log::error!("recording state could not be saved; recovery required");
                return Transition::Failed;
            }
        }
    }
    unreachable!("bounded recording retry returns on its final attempt")
}

#[cfg(test)]
async fn retry_for_test<F, Fut>(operation: F) -> Transition
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<bool, crate::error::AdapterError>>,
{
    retry_with(operation).await
}

fn digest(path: &Path) -> std::io::Result<(u64, String)> {
    let mut file = iptv_net::recording::open_retained(path)?;
    let bytes = file.metadata()?.len();
    if bytes > MAX_BYTES || bytes % 188 != 0 {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut read = 0u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        read = read.saturating_add(count as u64);
        if read > bytes {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        hash.update(&buffer[..count]);
    }
    if read != bytes {
        return Err(std::io::ErrorKind::UnexpectedEof.into());
    }
    Ok((bytes, format!("{:x}", hash.finalize())))
}

fn publish(services: &Services, completion: &RecordingCompletion) -> std::io::Result<()> {
    if !services.is_current() {
        return Err(std::io::ErrorKind::Interrupted.into());
    }
    let partial = path(services, completion.job, true);
    let complete = path(services, completion.job, false);
    let expected = (completion.bytes, completion.digest.clone());
    match digest(&complete) {
        Ok(actual) if actual == expected => {} // Lost acknowledgement after rename.
        Ok(_) => return Err(std::io::ErrorKind::AlreadyExists.into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if digest(&partial)? != expected {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
            if !services.is_current() {
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            finalize_name(&partial, &complete)?;
            // Revalidate the retained object before acknowledging it in the catalog.
            if digest(&complete)? != expected {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
        }
        Err(error) => return Err(error),
    }
    #[cfg(unix)]
    std::fs::File::open(media_directory(services.directory()))?.sync_all()?;
    Ok(())
}

pub(super) fn report_failure(
    services: &Services,
    result: Transition,
    notify: Option<crate::RecordingFailure>,
) -> Transition {
    if matches!(result, Transition::Applied)
        && let Some(reason) = notify
    {
        (services.ports().recording_failed)(reason);
    }
    result
}

async fn reconcile(
    services: &Services,
    completion: RecordingCompletion,
    notify_failure: Option<crate::RecordingFailure>,
) -> Transition {
    let files = services.clone();
    let receipt = completion.clone();
    if !matches!(
        services
            .spawn_blocking(move || publish(&files, &receipt))
            .await,
        Ok(Ok(()))
    ) {
        #[cfg(feature = "test-support")]
        host::observe_operation(
            services,
            completion.job,
            "completion media publication failed",
        );
        // A failed rename is a different failure from a lost catalog write. Keep
        // the verified spool playable as Partial, but never expose/overwrite an
        // unrelated regular file at the final path.
        let files = services.clone();
        let receipt = completion.clone();
        let retained = services
            .spawn_blocking(move || {
                let final_name = path(&files, receipt.job, false);
                let no_regular_destination = final_name
                    .symlink_metadata()
                    .map_or(true, |metadata| !metadata.is_file());
                no_regular_destination
                    && receipt.bytes > 0
                    && digest(&path(&files, receipt.job, true)).ok()
                        == Some((receipt.bytes, receipt.digest))
            })
            .await
            .unwrap_or(false);
        if retained {
            #[cfg(feature = "test-support")]
            let completion_job = completion.job;
            let result = retry(services, RecordingOperation::Retain(completion.clone())).await;
            #[cfg(feature = "test-support")]
            host::observe_operation(
                services,
                completion_job,
                match result {
                    Transition::Applied => "completion retained Partial",
                    Transition::Superseded => "completion retention superseded",
                    Transition::Failed => "completion retention failed",
                },
            );
            return report_failure(services, result, notify_failure);
        }
        #[cfg(feature = "test-support")]
        host::observe_operation(
            services,
            completion.job,
            "completion media retention unavailable",
        );
        log::error!("recording media could not be finalized; recovery required");
        return Transition::Failed;
    }
    #[cfg(feature = "test-support")]
    host::observe_operation(services, completion.job, "completion media published");
    let incomplete = completion.status != Status::Completed;
    #[cfg(feature = "test-support")]
    let completion_status = completion.status;
    #[cfg(feature = "test-support")]
    let completion_job = completion.job;
    let result = retry(services, RecordingOperation::Commit(completion.clone())).await;
    #[cfg(feature = "test-support")]
    host::observe_operation(
        services,
        completion_job,
        match result {
            Transition::Applied => match completion_status {
                Status::Completed => "completion committed Completed",
                Status::Partial => "completion committed Partial",
                Status::Failed => "completion committed Failed",
                _ => "completion committed another status",
            },
            Transition::Superseded => "completion commit superseded",
            Transition::Failed => "completion commit failed",
        },
    );
    report_failure(services, result, notify_failure.filter(|_| incomplete))
}

pub(super) async fn finish(
    services: &Services,
    job: &Job,
    outcome: Result<&'static str, &'static str>,
    notify_failure: Option<crate::RecordingFailure>,
) -> Transition {
    let partial = path(services, job.id, true);
    let evidence = services
        .spawn_blocking(move || {
            let evidence = digest(&partial)?;
            // File sync alone does not persist a newly-created directory entry.
            // Persist the spool name before the database can retain its receipt.
            #[cfg(unix)]
            std::fs::File::open(partial.parent().ok_or(std::io::ErrorKind::NotFound)?)?
                .sync_all()?;
            Ok::<_, std::io::Error>(evidence)
        })
        .await;
    let (bytes, digest) = match evidence {
        Ok(Ok(evidence)) => evidence,
        _ => {
            #[cfg(feature = "test-support")]
            host::observe_operation(services, job.id, "completion media inspection failed");
            let result = super::transition(
                services,
                job,
                Status::Failed,
                0,
                "Recording storage unavailable; retained media needs recovery",
            )
            .await;
            #[cfg(feature = "test-support")]
            host::observe_operation(
                services,
                job.id,
                match result {
                    Transition::Applied => "inspection failure committed Failed",
                    Transition::Superseded => "inspection failure superseded",
                    Transition::Failed => "inspection failure save failed",
                },
            );
            return report_failure(services, result, notify_failure);
        }
    };
    #[cfg(feature = "test-support")]
    host::observe_operation(services, job.id, "completion media inspected");
    let status = if bytes == 0 {
        Status::Failed
    } else if outcome.is_ok() {
        Status::Completed
    } else {
        Status::Partial
    };
    let completion = RecordingCompletion {
        job: job.id,
        status,
        bytes,
        digest,
        message: match outcome {
            Ok(_) if bytes == 0 => "No media was recorded",
            Ok(message) | Err(message) => message,
        }
        .into(),
    };
    let accepted = job.clone();
    let receipt = completion.clone();
    match retry(services, RecordingOperation::Prepare(accepted, receipt)).await {
        Transition::Applied => {
            #[cfg(feature = "test-support")]
            host::observe_operation(services, job.id, "completion intent prepared");
            reconcile(services, completion, notify_failure).await
        }
        other => {
            #[cfg(feature = "test-support")]
            host::observe_operation(
                services,
                job.id,
                match other {
                    Transition::Superseded => "completion preparation superseded",
                    Transition::Failed => "completion preparation failed",
                    Transition::Applied => unreachable!("handled applied completion"),
                },
            );
            other
        }
    }
}

/// None means legacy receipt-less media. A failed receipt is never downgraded to Partial.
pub(super) async fn recover(services: &Services, id: JobId) -> Option<Transition> {
    match services.recordings().completion(id).await {
        Ok(Some(receipt)) => Some(reconcile(services, receipt, None).await),
        Ok(None) => None,
        Err(_) => Some(Transition::Failed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};

    fn owner(root: &Path) -> Runtime {
        let owner = Runtime::new(crate::Ports::new(root.to_owned(), |_| {
            Ok(crate::model::DatabaseKey::from_bytes([0x75; 32]))
        }));
        owner.start_services();
        owner
    }

    async fn staged(root: &Path) -> (Runtime, Job, RecordingCompletion) {
        let (owner, mut request) = super::super::recovery_tests::fixture(root).await;
        request.start = now();
        request.end = now() + 60;
        let job = owner
            .services()
            .call(move |store| {
                let id = store.schedule_recording(JobId(801), &request, request.start)?;
                let mut job = store.recording_job(id)?.expect("generated job");
                assert!(store.claim_recording(&job)?);
                job.status = Status::Recording;
                Ok(job)
            })
            .await
            .unwrap();
        let directory = media_directory(root);
        std::fs::create_dir_all(directory).unwrap();
        let partial = path(&owner.services(), job.id, true);
        // Windows FlushFileBuffers requires a writable handle. Sync the same
        // descriptor that wrote the fixture, then close it before verification.
        let mut spool = std::fs::File::create(&partial).unwrap();
        spool.write_all(&[0x47; 188]).unwrap();
        spool.sync_all().unwrap();
        drop(spool);
        let (bytes, digest) = digest(&partial).unwrap();
        let receipt = RecordingCompletion {
            job: job.id,
            status: Status::Completed,
            bytes,
            digest,
            message: "Recording finished".into(),
        };
        (owner, job, receipt)
    }

    #[test]
    fn completion_crash_child() {
        let Ok(root) = std::env::var("PATINA_SYNTHETIC_COMPLETION_ROOT") else {
            return;
        };
        let stage = std::env::var("PATINA_SYNTHETIC_COMPLETION_STAGE").unwrap();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let (owner, job, receipt) = staged(Path::new(&root)).await;
                if stage != "spool" {
                    let prepared = receipt.clone();
                    assert!(
                        owner
                            .services()
                            .call(move |store| store.prepare_recording_completion(&job, &prepared))
                            .await
                            .unwrap()
                    );
                }
                if matches!(stage.as_str(), "rename" | "commit") {
                    publish(&owner.services(), &receipt).unwrap();
                }
                if stage == "commit" {
                    assert!(
                        owner
                            .services()
                            .call(move |store| store.commit_recording_completion(&receipt))
                            .await
                            .unwrap()
                    );
                }
                // Simulate process loss without running Runtime or worker destructors.
                std::process::exit(79);
            });
    }

    #[test]
    fn restart_reconciles_each_durable_completion_boundary_without_losing_success() {
        for stage in ["spool", "intent", "rename", "commit"] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().join("synthetic-profile");
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "recordings::completion::tests::completion_crash_child",
                    "--nocapture",
                ])
                .env("PATINA_SYNTHETIC_COMPLETION_ROOT", &root)
                .env("PATINA_SYNTHETIC_COMPLETION_STAGE", stage)
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("completion child timed out at {stage}");
                }
                std::thread::sleep(Duration::from_millis(5));
            };
            assert_eq!(status.code(), Some(79), "child stage {stage}");
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let owner = owner(&root);
                    let mut job = owner
                        .services()
                        .recordings()
                        .job(JobId(801))
                        .await
                        .unwrap()
                        .unwrap();
                    if stage != "commit" {
                        assert!(matches!(
                            super::super::recover_media(&owner.services(), &job).await,
                            Transition::Applied
                        ));
                    }
                    job = owner
                        .services()
                        .recordings()
                        .job(JobId(801))
                        .await
                        .unwrap()
                        .unwrap();
                    assert_eq!(
                        job.status,
                        if stage == "spool" {
                            Status::Partial
                        } else {
                            Status::Completed
                        },
                        "stage {stage}"
                    );
                    assert_eq!(job.bytes, 188);
                    assert_eq!(
                        std::fs::read(path(&owner.services(), job.id, false)).unwrap(),
                        [0x47; 188]
                    );
                    assert!(owner.drain_services());
                });
        }
    }

    #[tokio::test]
    async fn mismatched_retained_content_preserves_receipt_and_never_overwrites() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("profile");
        let (owner, job, receipt) = staged(&root).await;
        let accepted = receipt.clone();
        let expected = job.clone();
        owner
            .services()
            .call(move |store| store.prepare_recording_completion(&expected, &accepted))
            .await
            .unwrap();
        let destination = path(&owner.services(), job.id, false);
        std::fs::write(&destination, [0x48; 188]).unwrap();
        assert!(matches!(
            reconcile(&owner.services(), receipt.clone(), None).await,
            Transition::Failed
        ));
        assert_eq!(std::fs::read(destination).unwrap(), [0x48; 188]);
        assert!(
            owner
                .services()
                .recordings()
                .completion(job.id)
                .await
                .unwrap()
                .is_some()
        );
        assert!(owner.drain_services());
    }

    #[tokio::test]
    async fn bounded_retry_distinguishes_storage_full_from_superseded_state() {
        use std::sync::atomic::AtomicUsize;
        let directory = tempfile::tempdir().unwrap();
        let owner = owner(directory.path());
        for failure in [true, false] {
            let calls = Arc::new(AtomicUsize::new(0));
            let counted = calls.clone();
            let outcome = retry_for_test(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                let result = if failure {
                    Err(crate::error::AdapterError::Sqlite(
                        rusqlite::Error::SqliteFailure(
                            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
                            None,
                        ),
                    ))
                } else {
                    Ok(false)
                };
                std::future::ready(result)
            })
            .await;
            assert_eq!(calls.load(Ordering::SeqCst), if failure { 3 } else { 1 });
            assert!(matches!(
                (failure, outcome),
                (true, Transition::Failed) | (false, Transition::Superseded)
            ));
        }
        assert!(owner.drain_services());
    }
}

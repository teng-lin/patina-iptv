use super::*;
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source},
};
use std::io::{Read, Write};

#[test]
fn media_finalization_moves_once_and_never_replaces_an_existing_recording() {
    let directory = tempfile::tempdir().unwrap();
    let from = directory.path().join("generated.part");
    let to = directory.path().join("generated.ts");
    std::fs::write(&from, b"generated new media").unwrap();
    std::fs::write(&to, b"generated retained media").unwrap();
    assert!(finalize_name(&from, &to).is_err());
    assert_eq!(std::fs::read(&to).unwrap(), b"generated retained media");
    assert!(from.exists());
    std::fs::remove_file(&to).unwrap();
    finalize_name(&from, &to).unwrap();
    assert!(!from.exists());
    assert_eq!(std::fs::read(&to).unwrap(), b"generated new media");
}

fn owner(directory: &Path) -> Runtime {
    let owner = Runtime::new(crate::Ports::new(directory.to_owned(), |_| {
        Ok(iptv_store_sqlite::DatabaseKey::from_bytes([17; 32]))
    }));
    owner.start_services();
    owner
}
async fn seed(owner: &Runtime, url: &str) -> Request {
    let url = url.to_owned();
    owner
        .services()
        .call(move |store| {
            let source = store.add_source(&Source {
                name: "Synthetic recording source".into(),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: SecretUrl::new("https://example.test/generated.m3u"),
                    user_agent: None,
                    referrer: None,
                }),
            })?;
            let parsed = iptv_providers::m3u::parse_m3u(&format!(
                "#EXTM3U\n#EXTINF:-1 tvg-id=\"generated\",Generated channel\n{url}\n"
            ));
            let channels = parsed.channels.items;
            store.replace_catalog(source, &parsed.groups, &channels, &[], &[])?;
            Ok(Request {
                source: source.0,
                channel: channels[0].id.as_str().into(),
                channel_name: "Generated channel".into(),
                title: "Generated programme".into(),
                start: now() + 3600,
                end: now() + 7200,
            })
        })
        .await
        .unwrap()
}
fn server() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/generated.ts", listener.local_addr().unwrap());
    // Finite fixture, bounded even when a test fails before connecting.
    listener.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        let until = std::time::Instant::now() + Duration::from_secs(15);
        while std::time::Instant::now() < until {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut input = [0; 4096];
            let _ = stream.read(&mut input);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nConnection: close\r\n\r\n",
            );
            let mut packet = [0u8; 188 * 8];
            for part in packet.as_chunks_mut::<188>().0.iter_mut() {
                part[0] = 0x47;
                part[1] = 0x1f;
                part[2] = 0xff;
                part[3] = 0x10;
            }
            for _ in 0..1000 {
                if stream.write_all(&packet).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            break;
        }
    });
    url
}

#[tokio::test]
async fn a_reused_playlist_id_is_rejected_before_resolving_recording_media() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(directory.path());
    let request = seed(&owner, "https://example.test/generated.ts").await;
    let source = SourceId(request.source);
    let id = owner.schedule_recording(request.clone()).await.unwrap();
    owner
        .services()
        .call(move |store| store.delete_source(source))
        .await
        .unwrap();
    assert_eq!(
        seed(&owner, "https://example.test/generated.ts")
            .await
            .source,
        source.0
    );
    let job = owner
        .recording_jobs()
        .await
        .unwrap()
        .into_iter()
        .find(|j| j.id == id)
        .unwrap();
    assert!(
        resolve(
            &owner.services(),
            &owner.recordings,
            &job,
            &CancelToken::new()
        )
        .await
        .is_err()
    );
    let replacement = owner.schedule_recording(request.clone()).await.unwrap();
    assert_ne!(replacement, id);
    assert_eq!(
        owner.schedule_recording(request).await.unwrap(),
        replacement
    );
}
async fn until(owner: &Runtime, id: JobId, accept: impl Fn(&Job) -> bool) -> Job {
    for _ in 0..1000 {
        if let Some(job) = owner
            .recording_jobs()
            .await
            .unwrap()
            .into_iter()
            .find(|j| j.id == id)
            && accept(&job)
        {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "recording did not reach expected state: {:?}",
        owner.recording_jobs().await.unwrap()
    );
}

#[tokio::test]
async fn intent_is_idempotent_editable_and_cancelled_before_it_starts() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory.path().join("catalog"));
    let mut request = seed(&owner, "https://example.test/generated.ts").await;
    let id = owner.schedule_recording(request.clone()).await.unwrap();
    assert_eq!(owner.schedule_recording(request.clone()).await.unwrap(), id);
    request.end += 60;
    owner.update_recording(id, request).await.unwrap();
    owner.stop_recording(id).await.unwrap();
    until(&owner, id, |j| j.status == Status::Cancelled).await;
    assert!(owner.recording_file(id).await.is_err());
    owner.delete_recording(id).await.unwrap();
    assert!(owner.recording_jobs().await.unwrap().is_empty());
    assert!(owner.drain_services());
}

#[tokio::test]
async fn record_stop_play_and_delete_preserve_real_media_and_its_lease() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory.path().join("catalog"));
    let mut request = seed(&owner, &server()).await;
    request.start = now();
    request.end = now() + 20;
    let id = owner.schedule_recording(request).await.unwrap();
    until(&owner, id, |j| j.status == Status::Recording).await;
    for _ in 0..500 {
        if std::fs::metadata(path(&owner.services(), id, true)).is_ok_and(|m| m.len() >= 188) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    owner.stop_recording(id).await.unwrap();
    let job = until(&owner, id, |j| !j.status.active()).await;
    assert_eq!(job.status, Status::Completed, "{job:?}");
    let file = owner.recording_file(id).await.unwrap();
    assert!(std::fs::read(&file).unwrap().starts_with(&[0x47]));
    let source = owner.open_recording(id).await.unwrap();
    assert!(!source.public_only);
    let point = iptv_core::on_demand::Progress::observed(12_000, Some(60_000), false);
    owner.save_recording_progress(id, &source, point);
    assert_eq!(owner.recording_progress(id).await.unwrap(), point);
    let next = owner.open_recording(id).await.unwrap();
    let newer = iptv_core::on_demand::Progress::observed(18_000, Some(60_000), false);
    owner.save_recording_progress(id, &next, newer);
    owner.save_recording_progress(id, &source, point);
    assert_eq!(owner.recording_progress(id).await.unwrap(), newer);
    assert!(owner.delete_recording(id).await.is_err());
    owner.close_recording_playback();
    // Hold admission, queue deletion before a competing playback request, then
    // release it. A new reader cannot acquire a lease during the file awaits.
    let operation = owner.recordings.media_ops.lock().await;
    let mut deleting = Box::pin(owner.delete_recording(id));
    let mut opening = Box::pin(owner.open_recording(id));
    std::future::poll_fn(|cx| {
        use std::future::Future;
        assert!(deleting.as_mut().poll(cx).is_pending());
        assert!(opening.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    drop(operation);
    let (deleted, opened) = tokio::join!(deleting, opening);
    deleted.unwrap();
    assert!(opened.is_err());
    assert!(!file.exists());
    assert!(owner.drain_services());
}

#[tokio::test]
async fn startup_retries_a_transient_catalogue_failure_without_restarting_the_app() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("catalog");
    let owner = owner(&root);
    let request = seed(&owner, "https://example.test/generated.ts").await;
    let conn = rusqlite::Connection::open(root.join("iptv.sqlite")).unwrap();
    conn.pragma_update(None, "key", format!("x'{}'", "11".repeat(32)))
        .unwrap();
    conn.execute_batch("ALTER TABLE recording_jobs RENAME TO generated_unavailable_jobs")
        .unwrap();
    owner.start_recordings();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!owner.recordings.lock().ready);
    conn.execute_batch("ALTER TABLE generated_unavailable_jobs RENAME TO recording_jobs")
        .unwrap();
    drop(conn);
    let id = owner.schedule_recording(request).await.unwrap();
    assert!(owner.recordings.lock().ready);
    owner.stop_recording(id).await.unwrap();
    assert!(owner.drain_services());
}

#[tokio::test]
async fn a_failed_final_name_keeps_partial_media_playable_and_recoverable() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("catalog");
    let owner = owner(&root);
    let mut request = seed(&owner, &server()).await;
    request.start = now() + 2;
    request.end = now() + 20;
    let id = owner.schedule_recording(request).await.unwrap();
    let destination = path(&owner.services(), id, false);
    std::fs::create_dir_all(&destination).unwrap();
    until(&owner, id, |j| {
        j.status == Status::Recording && j.bytes >= 188
    })
    .await;
    owner.stop_recording(id).await.unwrap();
    let job = until(&owner, id, |j| j.status == Status::Partial).await;
    assert!(job.bytes >= 188);
    assert_eq!(
        owner.recording_file(id).await.unwrap().extension().unwrap(),
        "part"
    );
    owner.open_recording(id).await.unwrap();
    owner.close_recording_playback();
    std::fs::remove_dir(&destination).unwrap();
    recover(&owner.services()).await.unwrap();
    assert_eq!(owner.recording_file(id).await.unwrap(), destination);
    owner.delete_recording(id).await.unwrap();
}

#[tokio::test]
async fn restart_resumes_future_windows_and_never_claims_interrupted_media_is_complete() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("catalog");
    let first = owner(&root);
    let request = seed(&first, "https://example.test/generated.ts").await;
    let id = first.schedule_recording(request).await.unwrap();
    first
        .services()
        .call(move |s| {
            s.transition_recording(
                id,
                Status::Scheduled,
                Status::Recording,
                0,
                "Starting recording",
            )
        })
        .await
        .unwrap();
    std::fs::create_dir_all(media_directory(&root)).unwrap();
    std::fs::write(path(&first.services(), id, true), [0x47; 188]).unwrap();
    assert!(first.drain_services());
    drop(first);
    let second = owner(&root);
    second.start_recordings();
    let job = until(&second, id, |j| j.status == Status::Scheduled).await;
    assert_eq!(job.bytes, 188);
    assert!(path(&second.services(), id, true).is_file());
    assert!(second.recording_file(id).await.is_err());
    second.stop_recording(id).await.unwrap();
    let retained = until(&second, id, |job| job.status == Status::Partial).await;
    assert_eq!(retained.bytes, 188);
    assert!(second.recording_file(id).await.unwrap().is_file());
    assert!(second.drain_services());
}

#[test]
fn duplicate_accounts_share_connections_and_unknown_limits_are_conservative() {
    let mut data = Data::default();
    data.accounts.insert(
        1,
        Account::Xtream("https://example.test".into(), "generated".into()),
    );
    data.accounts.insert(2, data.accounts[&1].clone());
    data.limits.insert(1, 2);
    data.limits.insert(2, 2);
    data.viewers.insert(0, data.lease(1));
    assert!(data.capacity(2, None));
    data.active
        .insert(JobId(1), (data.lease(2), CancelToken::new()));
    assert!(!data.capacity(1, None));
    assert!(
        data.capacity(1, Some(0)),
        "a retry retains the surface's lease"
    );
    data.provider_limits.insert(2, 1);
    assert!(!data.capacity(1, Some(0)));
    data.accounts.insert(3, Account::Unknown);
    assert!(
        !data.capacity(3, None),
        "unknown account association is conservative"
    );
    data.active.clear();
    data.viewers.clear();
    assert!(data.capacity(3, None));
    data.viewers.insert(1, data.lease(3));
    assert!(!data.capacity(3, None));
}

#[tokio::test]
async fn a_connection_conflict_fails_recording_without_stopping_the_viewer() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory.path().join("catalog"));
    let mut request = seed(&owner, "https://example.test/generated.ts").await;
    assert!(owner.reserve_viewing(0, SourceId(request.source)));
    request.start = now() + 2;
    request.end = now() + 20;
    let id = owner.schedule_recording(request).await.unwrap();
    let job = until(&owner, id, |j| j.status == Status::Failed).await;
    assert!(job.message.contains("connection"));
    assert_eq!(owner.recordings.lock().viewers.len(), 1);
    assert!(owner.drain_services());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_host_captures_after_its_ui_owner_is_drained_and_dropped() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("generated-app");
    std::fs::create_dir_all(&root).unwrap();
    let mut ports = crate::Ports::new(root.clone(), |_| {
        Ok(iptv_store_sqlite::DatabaseKey::from_bytes([17; 32]))
    });
    ports.background_recordings = true;
    let owner = Runtime::new(ports.clone());
    owner.start_services();
    let mut request = seed(&owner, &server()).await;
    request.start = now() + 2;
    request.end = request.start + 3;
    let id = owner.schedule_recording(request).await.unwrap();
    let state = owner.recordings.clone();
    assert!(owner.drain_services());
    drop(owner);
    let services = state.host.lock().unwrap().as_ref().unwrap().accept();
    tokio::time::sleep(Duration::from_secs(7)).await;
    let job = services
        .read(move |store| store.recording_job(id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.status, Status::Completed);
    assert!(job.bytes >= 188);
    assert!(path(&services, id, false).is_file());
    assert!(state.lock().viewers.is_empty());
    state.stop();
}

#[test]
fn mobile_handoff_reserves_all_returning_surfaces_before_any_reconnect() {
    let directory = tempfile::tempdir().unwrap();
    let mut ports = crate::Ports::new(directory.path().join("generated-app"), |_| {
        Ok(iptv_store_sqlite::DatabaseKey::from_bytes([17; 32]))
    });
    ports.background_recordings = true;
    let state = host::acquire(&ports);
    {
        let mut data = state.lock();
        data.accounts.insert(1, Account::Unknown);
        data.limits.insert(1, 2);
        let lease = data.lease(1);
        data.viewers.insert(0, lease.clone());
        data.viewers.insert(1, lease);
    }
    host::suspend_viewing(&ports.directory);
    assert!(state.lock().viewers.is_empty());
    {
        let mut data = state.lock();
        let lease = data.lease(1);
        data.active.insert(JobId(90), (lease, CancelToken::new()));
    }
    assert!(!host::resume_viewing(&ports.directory));
    assert!(
        state.lock().viewers.is_empty(),
        "a partial return must roll back before native reconnect"
    );
    state.lock().active.clear();
    assert!(host::resume_viewing(&ports.directory));
    assert_eq!(state.lock().viewers.len(), 2);
}
#[test]
fn a_drained_ui_cannot_release_its_successors_viewing_lease() {
    let directory = tempfile::tempdir().unwrap();
    let mut ports = crate::Ports::new(directory.path().join("generated-app"), |_| {
        Ok(iptv_store_sqlite::DatabaseKey::from_bytes([17; 32]))
    });
    ports.background_recordings = true;
    let old = Runtime::new(ports.clone());
    old.start_services();
    old.reserve_viewing(0, SourceId(1));
    old.drain_services();
    let new = Runtime::new(ports);
    new.start_services();
    assert!(new.reserve_viewing(0, SourceId(1)));
    old.release_viewing(0);
    drop(old);
    assert!(new.recordings.lock().viewers.contains_key(&0));
    new.drain_services();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cold_native_host_resumes_a_spool_without_constructing_another_ui() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("generated-app");
    let first = owner(&root);
    let mut request = seed(&first, &server()).await;
    request.start = now() + 2;
    request.end = request.start + 3;
    let id = first.schedule_recording(request).await.unwrap();
    assert!(first.drain_services());
    drop(first);
    // Synthetic interrupted-process state: an acknowledged Recording row and
    // a complete packet followed by a torn packet in its private spool.
    let mut store = iptv_store_sqlite::Store::open_encrypted(
        root.join("iptv.sqlite"),
        &iptv_store_sqlite::DatabaseKey::from_bytes([17; 32]),
    )
    .unwrap();
    assert!(
        store
            .transition_recording(
                id,
                Status::Scheduled,
                Status::Recording,
                0,
                "Starting recording"
            )
            .unwrap()
    );
    drop(store);
    std::fs::create_dir_all(media_directory(&root)).unwrap();
    let spool = media_directory(&root).join(format!("{}.part", id.0));
    std::fs::write(&spool, [0x47; 200]).unwrap();
    let mut ports = crate::Ports::new(root.clone(), |_| {
        Ok(iptv_store_sqlite::DatabaseKey::from_bytes([17; 32]))
    });
    ports.background_recordings = true;
    host::start_background_host(ports.clone()).unwrap();
    let state = host::acquire(&ports);
    let services = state.host.lock().unwrap().as_ref().unwrap().accept();
    tokio::time::sleep(Duration::from_secs(7)).await;
    let job = services
        .read(move |store| store.recording_job(id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        job.status,
        Status::Partial,
        "a recovered gap must not be presented as complete"
    );
    assert!(
        job.bytes > 188,
        "capture must append new media after process restart"
    );
    assert_eq!(job.bytes % 188, 0);
    assert!(!spool.exists());
    assert!(path(&services, id, false).is_file());
    assert!(state.stop());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_surviving_ui_and_a_restarted_host_keep_one_recording_budget() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("generated-app");
    let mut ports = crate::Ports::new(root.clone(), |_| {
        Ok(iptv_store_sqlite::DatabaseKey::from_bytes([17; 32]))
    });
    ports.background_recordings = true;
    let owner = Runtime::new(ports.clone());
    owner.start_services();
    let mut request = seed(&owner, &server()).await;
    owner.start_recordings();
    for _ in 0..100 {
        if owner.recordings.lock().ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(owner.reserve_viewing(0, SourceId(request.source)));
    let first_services = owner
        .recordings
        .host
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .accept();
    assert!(owner.shutdown_recording_host());
    assert!(
        !first_services.is_current(),
        "old store epoch must close before a successor opens"
    );
    // The OS restarts its native service while this exact UI Runtime survives.
    host::start_background_host(ports.clone()).unwrap();
    let restarted = host::acquire(&ports);
    assert!(Arc::ptr_eq(&owner.recordings, &restarted));
    assert!(restarted.lock().viewers.contains_key(&0));
    owner.release_viewing(0);
    assert!(restarted.lock().viewers.is_empty());
    request.start = now();
    request.end = request.start + 20;
    let id = owner.schedule_recording(request.clone()).await.unwrap();
    until(&owner, id, |job| {
        job.status == Status::Recording && job.bytes >= 188
    })
    .await;
    assert!(
        !owner.reserve_viewing(0, SourceId(request.source)),
        "a surviving UI cannot bypass its restarted host's active recording lease"
    );
    owner.stop_recording(id).await.unwrap();
    let job = until(&owner, id, |job| !job.status.active()).await;
    assert!(job.bytes >= 188);
    assert!(owner.shutdown_recording_host());
    assert!(owner.drain_services());
}

#[tokio::test]
async fn enabling_a_rule_publishes_background_intent_before_it_returns() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory.path().join("generated-app"));
    let request = seed(&owner, "https://example.test/generated.ts").await;
    let id = owner
        .schedule_repeating_recording(request, iptv_core::recording::Repeat::Daily)
        .await
        .unwrap();
    let rule = owner.recording_rules().await.unwrap()[0].id;
    owner.set_recording_rule_enabled(rule, false).await.unwrap();
    assert!(!owner.recording_background_needed());
    owner.set_recording_rule_enabled(rule, true).await.unwrap();
    assert!(owner.recording_background_needed());
    assert!(
        owner
            .recording_snapshot()
            .iter()
            .any(|job| job.id == id && job.status == Status::Scheduled)
    );
    assert!(owner.drain_services());
}

#[derive(Default)]
struct PublicationGate {
    entered: AtomicBool,
    released: Mutex<bool>,
    changed: Condvar,
}
static PUBLICATION_GATE: Mutex<Option<Arc<PublicationGate>>> = Mutex::new(None);
fn blocking_wake_registration(next: i64, _active: bool) -> iptv_core::recording::WakeCapability {
    use iptv_core::recording::WakeCapability;
    if next <= 0 {
        return WakeCapability::Exact;
    }
    let gate = PUBLICATION_GATE.lock().unwrap().clone();
    if let Some(gate) = gate {
        gate.entered.store(true, Ordering::SeqCst);
        let released = gate.released.lock().unwrap();
        let (released, _) = gate
            .changed
            .wait_timeout_while(released, Duration::from_secs(10), |released| !*released)
            .unwrap();
        if !*released {
            return WakeCapability::Denied;
        }
    }
    WakeCapability::Exact
}
struct ReleasePublication(Arc<PublicationGate>);
impl Drop for ReleasePublication {
    fn drop(&mut self) {
        *self.0.released.lock().unwrap() = true;
        self.0.changed.notify_all();
        *PUBLICATION_GATE.lock().unwrap() = None;
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_schedule_waits_for_os_registration_without_blocking_the_ui_executor() {
    let directory = tempfile::tempdir().unwrap();
    let gate = Arc::new(PublicationGate::default());
    *PUBLICATION_GATE.lock().unwrap() = Some(gate.clone());
    let release = ReleasePublication(gate.clone());
    let mut ports = crate::Ports::new(directory.path().join("generated-app"), |_| {
        Ok(iptv_store_sqlite::DatabaseKey::from_bytes([17; 32]))
    });
    ports.recording_schedule = blocking_wake_registration;
    let owner = Runtime::new(ports);
    owner.start_services();
    let request = seed(&owner, "https://example.test/generated.ts").await;
    let mut accepted = Box::pin(owner.schedule_recording(request));
    tokio::select! {
        result=&mut accepted=>panic!("schedule acknowledged before platform registration completed: {result:?}"),
        _=async {while !gate.entered.load(Ordering::SeqCst) {tokio::time::sleep(Duration::from_millis(10)).await;}}=>{},
        _=tokio::time::sleep(Duration::from_secs(3))=>panic!("platform registration did not begin"),
    }
    assert_eq!(
        owner
            .services()
            .read(|store| store.recording_jobs())
            .await
            .unwrap()
            .len(),
        1,
        "intent is durable while platform registration is still blocked"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), accepted.as_mut())
            .await
            .is_err(),
        "the UI executor must remain responsive while acceptance stays pending"
    );
    drop(release);
    let id = accepted.await.unwrap();
    assert_eq!(owner.recording_snapshot()[0].id, id);
    assert_eq!(owner.recording_wake_notice(), None);
    assert!(owner.drain_services());
}
#[tokio::test]
async fn a_denied_wake_is_explicit_and_does_not_discard_the_accepted_job() {
    let directory = tempfile::tempdir().unwrap();
    let mut ports = crate::Ports::new(directory.path().join("generated-app"), |_| {
        Ok(iptv_store_sqlite::DatabaseKey::from_bytes([17; 32]))
    });
    ports.recording_schedule = |_, _| iptv_core::recording::WakeCapability::Denied;
    let owner = Runtime::new(ports);
    owner.start_services();
    let request = seed(&owner, "https://example.test/generated.ts").await;
    let id = owner.schedule_recording(request).await.unwrap();
    assert!(owner.recording_wake_notice().is_some());
    assert_eq!(
        owner
            .recording_jobs()
            .await
            .unwrap()
            .iter()
            .find(|job| job.id == id)
            .unwrap()
            .status,
        Status::Scheduled
    );
    assert!(owner.drain_services());
}

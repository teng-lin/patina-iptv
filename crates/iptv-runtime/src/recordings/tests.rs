use super::*;
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source},
};
use std::io::{Read, Write};

#[test]
fn an_idle_unknown_playlist_does_not_lower_a_known_accounts_allowance() {
    let mut data = Data::default();
    data.accounts.insert(
        1,
        Account::Xtream("https://example.test".into(), "generated".into()),
    );
    data.provider_limits.insert(1, 2);
    data.limits.insert(1, 2);
    data.accounts.insert(2, Account::Unknown);
    data.limits.insert(2, 1);
    data.viewers.insert(0, data.lease(1));
    assert!(
        data.capacity(1, None),
        "an idle M3U playlist is not a one-stream cap on Xtream"
    );
    data.viewers.insert(1, data.lease(1));
    assert!(
        !data.capacity(1, None),
        "the known two-stream allowance still applies"
    );
    data.viewers.remove(&1);
    data.viewers.insert(1, data.lease(2));
    assert!(
        !data.capacity(1, Some(0)),
        "an active unknown-account stream still shares the conservative budget"
    );
}

#[test]
fn registered_unknown_inputs_keep_their_limit_without_a_viewer() {
    for closing in [false, true] {
        let mut data = Data::default();
        data.accounts.insert(
            1,
            Account::Xtream("https://example.test".into(), "generated".into()),
        );
        data.provider_limits.insert(1, 2);
        data.limits.insert(1, 2);
        data.accounts.insert(2, Account::Unknown);
        data.limits.insert(2, 1);
        let mut lease = data.lease(2);
        lease.pending = Some(7);
        data.inputs.insert(7, shared::Registered { lease, closing });
        assert!(
            !data.capacity(1, None),
            "a pending or closing input still owns its upstream slot"
        );
        data.inputs.clear();
        assert!(
            data.capacity(1, None),
            "capacity returns after acknowledged retirement"
        );
    }
}

#[test]
fn an_idle_known_account_does_not_lower_an_unknown_playlists_configured_limit() {
    let mut data = Data::default();
    data.accounts.insert(
        1,
        Account::Xtream("https://example.test".into(), "generated".into()),
    );
    data.provider_limits.insert(1, 1);
    data.limits.insert(1, 1);
    data.accounts.insert(2, Account::Unknown);
    data.limits.insert(2, 2);
    data.viewers.insert(0, data.lease(2));
    assert!(
        data.capacity(2, None),
        "an idle known account does not prove an alias for M3U"
    );
    data.viewers.insert(1, data.lease(1));
    assert!(
        !data.capacity(2, Some(0)),
        "an occupied known account still shares the conservative budget"
    );
}

#[test]
fn a_reported_connection_allowance_is_not_masked_by_the_default() {
    let mut settings = iptv_settings::Settings::new();
    assert_eq!(connection_limit(&settings, 1, Some(3)), 3);
    assert_eq!(connection_limit(&settings, 1, None), 1);
    assert_eq!(connection_limit(&settings, 1, Some(0)), 1);
    settings
        .set_in(
            "playlists.max_connections",
            &iptv_settings::Target::playlist("1"),
            iptv_settings::Value::Int(1),
        )
        .unwrap();
    assert_eq!(connection_limit(&settings, 1, Some(3)), 1);
    assert_eq!(connection_limit(&settings, 2, Some(3)), 3);
    // A manual value cannot override the provider's lower hard limit.
    let mut data = Data::default();
    data.accounts.insert(1, Account::Unknown);
    data.provider_limits.insert(1, 1);
    data.limits.insert(1, 3);
    data.viewers.insert(0, data.lease(1));
    assert!(!data.capacity(1, None));
}

#[test]
fn a_queued_registration_cannot_outlive_its_application_owner() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    let directory = tempfile::tempdir().unwrap();
    let mut ports = crate::Ports::new(directory.path().to_owned(), |_| {
        panic!("a wake report must not open the catalog")
    });
    ports.recording_schedule = |_| {
        CALLS.fetch_add(1, Ordering::SeqCst);
    };
    let owner = Runtime::new(ports);
    owner.start_services();
    owner.recordings.lock().ready = true;
    let state = owner.recordings.clone();
    let publication = state.publication.lock().unwrap();
    let queued = state.clone();
    let services = owner.services();
    let worker = std::thread::spawn(move || queued.publish_schedule(&services, 0, true));
    assert!(owner.drain_services());
    drop(publication);
    assert!(!worker.join().unwrap());
    assert_eq!(CALLS.load(Ordering::SeqCst), 0);
}

#[test]
fn a_registration_spanning_native_restart_cannot_publish_the_old_report() {
    use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
    type Gate = (SyncSender<()>, Receiver<()>);
    static GATE: std::sync::Mutex<Option<Gate>> = std::sync::Mutex::new(None);
    fn register(_: bool) {
        let (entered, release) = GATE.lock().unwrap().take().unwrap();
        entered.send(()).unwrap();
        release.recv_timeout(Duration::from_secs(10)).unwrap();
    }
    let directory = tempfile::tempdir().unwrap();
    let mut ports = crate::Ports::new(directory.path().to_owned(), |_| {
        panic!("a wake report must not open the catalog")
    });
    ports.recording_schedule = register;
    let owner = Runtime::new(ports);
    owner.start_services();
    owner.recordings.lock().ready = true;
    let (entered, seen) = sync_channel(1);
    let (release, parked) = sync_channel(1);
    *GATE.lock().unwrap() = Some((entered, parked));
    let state = owner.recordings.clone();
    let services = owner.services();
    let worker = std::thread::spawn(move || state.publish_schedule(&services, 0, true));
    seen.recv_timeout(Duration::from_secs(10)).unwrap();
    // The execution gate changes this identity before a successor can load.
    owner.recordings.execution().generation += 1;
    release.send(()).unwrap();
    assert!(!worker.join().unwrap());
    assert_eq!(owner.recordings.lock().published_schedule, None);
    assert!(owner.drain_services());
}

#[tokio::test]
async fn the_host_is_told_whether_a_capture_is_due_and_a_late_poll_cannot_replace_it() {
    static PUBLISHED: std::sync::Mutex<Vec<bool>> = std::sync::Mutex::new(Vec::new());
    fn publish(active: bool) {
        PUBLISHED.lock().unwrap().push(active);
    }
    let directory = tempfile::tempdir().unwrap();
    let mut ports = crate::Ports::new(directory.path().to_owned(), |_| {
        panic!("a publication must not open the catalog")
    });
    ports.recording_schedule = publish;
    let owner = Runtime::new(ports);
    owner.start_services();
    let at = now();
    // A job an earlier version left for later is not a capture to run for.
    let mut job = Job {
        id: JobId(1),
        source_current: true,
        series_start: None,
        request: Request {
            source: 1,
            channel: "generated".into(),
            channel_name: "Generated channel".into(),
            title: "Generated program".into(),
            start: at + 86400,
            end: at + 86460,
        },
        status: Status::Scheduled,
        bytes: 0,
        message: String::new(),
    };
    {
        let mut data = owner.recordings.lock();
        data.ready = true;
        data.jobs = vec![job.clone()];
    }
    assert!(
        owner
            .recordings
            .publish_schedule(&owner.services(), 0, false)
    );
    assert_eq!(*PUBLISHED.lock().unwrap(), [false]);
    // An unchanged poll does not call the host again.
    assert!(
        owner
            .recordings
            .publish_schedule(&owner.services(), 0, false)
    );
    assert_eq!(PUBLISHED.lock().unwrap().len(), 1);
    job.request.start = at - 1;
    job.request.end = at + 60;
    {
        let mut data = owner.recordings.lock();
        data.intent_revision += 1;
        data.jobs = vec![job];
    }
    owner.publish_recording_schedule().await.unwrap();
    assert_eq!(PUBLISHED.lock().unwrap().last(), Some(&true));
    assert!(
        !owner
            .recordings
            .publish_schedule(&owner.services(), 0, false)
    );
    assert_eq!(
        PUBLISHED.lock().unwrap().len(),
        2,
        "a late native poll cannot replace the newer capture"
    );
    assert!(owner.drain_services());
}

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

pub(super) fn owner(directory: &Path) -> Runtime {
    let owner = Runtime::new(crate::Ports::new(directory.to_owned(), |_| {
        Ok(iptv_store_sqlite::DatabaseKey::from_bytes([17; 32]))
    }));
    owner.start_services();
    owner
}
pub(super) async fn seed(owner: &Runtime, url: &str) -> Request {
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
                title: "Generated program".into(),
                start: now() + 3600,
                end: now() + 7200,
            })
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn record_now_uses_the_reported_spare_connection_and_preserves_viewing() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory.path().join("generated-app"));
    let mut request = seed(&owner, &server()).await;
    owner
        .recordings
        .lock()
        .provider_limits
        .insert(request.source, 3);
    refresh_accounts(&owner.services(), &owner.recordings)
        .await
        .unwrap();
    assert!(owner.reserve_viewing(0, SourceId(request.source)));
    request.start = now();
    request.end = request.start + 20;
    let id = owner.schedule_recording(request).await.unwrap();
    until(&owner, id, |job| {
        job.status == Status::Recording && job.bytes > 0
    })
    .await;
    assert_eq!(owner.recordings.lock().viewers.len(), 1);
    owner.stop_recording(id).await.unwrap();
    let job = until(&owner, id, |job| job.status == Status::Completed).await;
    assert!(job.bytes >= 188);
    assert_eq!(owner.recordings.lock().viewers.len(), 1);
    assert!(owner.drain_services());
}

#[tokio::test]
async fn record_now_says_a_full_library_before_it_starts() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("generated-app");
    let owner = owner(&root);
    let mut request = seed(&owner, &server()).await;
    // A sparse file takes the library to its budget without using the disk.
    let media = media_directory(&root);
    std::fs::create_dir_all(&media).unwrap();
    std::fs::File::create(media.join("1.ts"))
        .unwrap()
        .set_len(iptv_core::recording::LIBRARY_BYTES - iptv_core::recording::MAX_BYTES + 188)
        .unwrap();
    request.start = now();
    request.end = request.start + 20;
    let error = owner.schedule_recording(request.clone()).await.unwrap_err();
    assert_eq!(
        error.unavailable_reason(),
        Some("The recordings library is full. Delete recordings to record more.")
    );
    assert!(
        owner.recording_snapshot().is_empty(),
        "nothing was accepted"
    );
    // Room again: the same request starts.
    std::fs::remove_file(media.join("1.ts")).unwrap();
    let id = owner.schedule_recording(request).await.unwrap();
    owner.stop_recording(id).await.unwrap();
    assert!(owner.drain_services());
}

#[test]
fn space_admission_counts_a_library_not_created_yet_as_empty() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("generated-app");
    std::fs::create_dir_all(&root).unwrap();
    assert_eq!(space_admission(&root, &[], 60), Ok(()));
    // A day at the estimated rate is capped at one recording's maximum,
    // which a real temporary directory may or may not hold: either answer
    // is the free-space check, never a storage failure.
    if let Err(reason) = space_admission(&root, &[], iptv_core::recording::MAX_DURATION) {
        assert!(reason.contains("free space"), "{reason}");
    }
}

#[cfg(windows)]
#[test]
fn recording_storage_queries_free_space_on_the_windows_data_volume() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("Generated recording library");
    std::fs::create_dir(&root).unwrap();
    assert!(available_space(&root).is_some());
    assert_eq!(available_space(&root.join("missing")), None);
    let truncated = directory.path().join("Generated recording library\0suffix");
    assert_eq!(available_space(&truncated), None);
}

#[tokio::test]
async fn a_capture_the_host_stops_early_keeps_its_partial_file_and_the_reason() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory.path().join("generated-app"));
    let mut request = seed(&owner, &server()).await;
    request.start = now();
    request.end = request.start + 60;
    let id = owner.schedule_recording(request).await.unwrap();
    until(&owner, id, |job| {
        job.status == Status::Recording && job.bytes > 0
    })
    .await;
    assert_eq!(owner.stop_captures(host::EarlyStop::TvOff), 1);
    let job = until(&owner, id, |job| job.status == Status::Partial).await;
    assert!(job.bytes >= 188, "the partial file is kept");
    assert_eq!(job.message, host::EarlyStop::TvOff.reason());
    assert!(owner.recordings.lock().stop_reasons.is_empty());
    assert!(owner.drain_services());
}

#[test]
fn repeated_host_stops_skip_captures_already_closing() {
    let state = State::default();
    let stopped_by_viewer = JobId(41);
    let stopped_by_host = JobId(42);
    {
        let mut data = state.lock();
        let viewer_stop = CancelToken::new();
        viewer_stop.cancel();
        let lease = data.lease(1);
        data.active
            .insert(stopped_by_viewer, (lease.clone(), viewer_stop));
        data.active
            .insert(stopped_by_host, (lease, CancelToken::new()));
    }
    assert_eq!(state.stop_captures(host::EarlyStop::TvOff), 1);
    assert_eq!(state.stop_captures(host::EarlyStop::TvOff), 0);
    assert_eq!(state.stop_captures(host::EarlyStop::BackgroundLimit), 0);
    let data = state.lock();
    assert!(data.active.values().all(|(_, stop)| stop.is_canceled()));
    assert!(!data.stop_reasons.contains_key(&stopped_by_viewer));
    assert_eq!(
        data.stop_reasons.get(&stopped_by_host),
        Some(&host::EarlyStop::TvOff.reason())
    );
}

#[tokio::test]
async fn tv_standby_rejects_new_and_recovered_captures_without_losing_the_spool() {
    static NOTICES: Mutex<Vec<crate::RecordingFailure>> = Mutex::new(Vec::new());
    for recovering in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("generated-app");
        let mut ports = crate::Ports::new(root.clone(), |_| {
            Ok(iptv_store_sqlite::DatabaseKey::from_bytes([17; 32]))
        });
        ports.recording_standby = Some(|| true);
        ports.recording_failed = |reason| NOTICES.lock().unwrap().push(reason);
        let owner = Runtime::new(ports);
        owner.start_services();
        let mut request = seed(&owner, "https://example.test/generated.ts").await;
        request.start = now();
        request.end = request.start + 60;
        let id = if recovering {
            let id = JobId(909);
            std::fs::create_dir_all(media_directory(&root)).unwrap();
            std::fs::write(path(&owner.services(), id, true), [0x47; 188]).unwrap();
            owner
                .services()
                .call(move |store| {
                    store.schedule_recording(id, &request, request.start)?;
                    let recording = store.recording_job(id)?.expect("generated job");
                    assert!(store.claim_recording(&recording)?);
                    Ok(())
                })
                .await
                .unwrap();
            owner.start_recordings();
            id
        } else {
            owner.schedule_recording(request).await.unwrap()
        };

        let job = until(&owner, id, |job| !job.status.active()).await;
        assert_eq!(job.message, host::EarlyStop::TvOff.reason());
        if recovering {
            assert_eq!((job.status, job.bytes), (Status::Partial, 188));
            assert_eq!(
                std::fs::read(owner.recording_file(id).await.unwrap()).unwrap(),
                [0x47; 188]
            );
        } else {
            assert_eq!((job.status, job.bytes), (Status::Failed, 0));
        }
        assert!(!path(&owner.services(), id, true).exists());
        assert!(owner.drain_services());
        assert_eq!(
            std::mem::take(&mut *NOTICES.lock().unwrap()),
            [crate::RecordingFailure::TvStandby]
        );
    }
}

#[tokio::test]
async fn a_capture_that_fails_to_open_notifies_the_host_once() {
    static NOTICES: Mutex<Vec<crate::RecordingFailure>> = Mutex::new(Vec::new());
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("generated-app");
    let mut ports = crate::Ports::new(root.clone(), |_| {
        Ok(iptv_store_sqlite::DatabaseKey::from_bytes([17; 32]))
    });
    ports.recording_failed = |reason| NOTICES.lock().unwrap().push(reason);
    let owner = Runtime::new(ports);
    owner.start_services();
    let mut request = seed(&owner, "https://example.test/generated.ts").await;
    // The accepted job cannot open a spool if a file blocks its media directory.
    std::fs::write(media_directory(&root), b"generated obstructing file").unwrap();
    request.start = now();
    request.end = request.start + 60;
    let id = JobId(910);
    owner
        .services()
        .call(move |store| store.schedule_recording(id, &request, request.start))
        .await
        .unwrap();
    owner.start_recordings();
    let job = until(&owner, id, |job| job.status == Status::Failed).await;
    assert!(job.message.contains("storage unavailable"));
    assert!(owner.drain_services());
    assert_eq!(
        *NOTICES.lock().unwrap(),
        [crate::RecordingFailure::Unexpected]
    );
}

#[tokio::test]
async fn an_expired_active_capture_keeps_its_media_and_reports_recovery_once() {
    use std::sync::atomic::AtomicUsize;
    static NOTICES: AtomicUsize = AtomicUsize::new(0);
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("generated-app");
    let mut ports = crate::Ports::new(root.clone(), |_| {
        Ok(iptv_store_sqlite::DatabaseKey::from_bytes([17; 32]))
    });
    ports.recording_failed = |reason| {
        assert_eq!(reason, crate::RecordingFailure::Unexpected);
        NOTICES.fetch_add(1, Ordering::Relaxed);
    };
    let owner = Runtime::new(ports);
    owner.start_services();
    let mut request = seed(&owner, "https://example.test/generated.ts").await;
    request.start = now() - 120;
    request.end = now() - 60;
    let id = JobId(911);
    std::fs::create_dir_all(media_directory(&root)).unwrap();
    std::fs::write(path(&owner.services(), id, true), [0x47; 188]).unwrap();
    owner
        .services()
        .call(move |store| {
            store.schedule_recording(id, &request, request.start)?;
            let recording = store.recording_job(id)?.expect("generated job");
            assert!(store.claim_recording(&recording)?);
            Ok(())
        })
        .await
        .unwrap();

    // Startup recovery is idempotent even if it sees the same terminal row again.
    recover(&owner.services()).await.unwrap();
    recover(&owner.services()).await.unwrap();
    let job = until(&owner, id, |job| job.status == Status::Partial).await;
    assert_eq!(job.bytes, 188);
    assert_eq!(job.message, "Interrupted when the app closed");
    let media = owner.recording_file(id).await.unwrap();
    assert_eq!(std::fs::read(media).unwrap(), [0x47; 188]);
    assert!(owner.drain_services());
    assert_eq!(NOTICES.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn a_reused_playlist_id_does_not_inherit_an_old_connection_allowance() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory.path().join("generated-app"));
    let request = seed(&owner, "https://example.test/generated.ts").await;
    owner
        .recordings
        .lock()
        .provider_limits
        .insert(request.source, 3);
    refresh_accounts(&owner.services(), &owner.recordings)
        .await
        .unwrap();
    assert_eq!(owner.recordings.lock().limits[&request.source], 3);
    let source = SourceId(request.source);
    owner
        .services()
        .call(move |store| store.delete_source(source))
        .await
        .unwrap();
    let replacement = seed(&owner, "https://example.test/generated.ts").await;
    assert_eq!(replacement.source, request.source);
    refresh_accounts(&owner.services(), &owner.recordings)
        .await
        .unwrap();
    assert_eq!(owner.recordings.lock().limits[&replacement.source], 1);
    assert!(owner.drain_services());
}

#[tokio::test]
async fn an_isolated_recording_owner_can_restart_without_a_native_background_service() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory.path().join("generated-app"));
    let mut request = seed(&owner, &server()).await;
    owner.start_recordings();
    for _ in 0..100 {
        if owner.recordings.lock().ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(owner.shutdown_recording_host());
    request.start = now();
    request.end = request.start + 20;
    let id = owner.schedule_recording(request).await.unwrap();
    until(&owner, id, |job| {
        job.status == Status::Recording && job.bytes >= 188
    })
    .await;
    owner.stop_recording(id).await.unwrap();
    until(&owner, id, |job| !job.status.active()).await;
    assert!(owner.drain_services());
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
pub(super) async fn until(owner: &Runtime, id: JobId, accept: impl Fn(&Job) -> bool) -> Job {
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
async fn intent_is_idempotent_and_canceled_before_it_starts() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory.path().join("catalog"));
    let request = seed(&owner, "https://example.test/generated.ts").await;
    let id = owner.schedule_recording(request.clone()).await.unwrap();
    assert_eq!(owner.schedule_recording(request).await.unwrap(), id);
    owner.stop_recording(id).await.unwrap();
    until(&owner, id, |j| j.status == Status::Canceled).await;
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
async fn startup_retries_a_transient_catalog_failure_without_restarting_the_app() {
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
    use std::sync::atomic::AtomicUsize;
    static NOTICES: AtomicUsize = AtomicUsize::new(0);
    let directory = tempfile::tempdir().unwrap();
    let mut ports = crate::Ports::new(directory.path().join("catalog"), |_| {
        Ok(iptv_store_sqlite::DatabaseKey::from_bytes([17; 32]))
    });
    ports.recording_failed = |reason| {
        assert_eq!(reason, crate::RecordingFailure::Unexpected);
        NOTICES.fetch_add(1, Ordering::Relaxed);
    };
    let owner = Runtime::new(ports);
    owner.start_services();
    let mut request = seed(&owner, "https://example.test/generated.ts").await;
    assert!(owner.reserve_viewing(0, SourceId(request.source)));
    request.start = now() + 2;
    request.end = now() + 20;
    let id = owner.schedule_recording(request).await.unwrap();
    let job = until(&owner, id, |j| j.status == Status::Failed).await;
    assert!(job.message.contains("connection"));
    assert_eq!(owner.recordings.lock().viewers.len(), 1);
    assert!(owner.drain_services());
    assert_eq!(NOTICES.load(Ordering::Relaxed), 1);
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
    // Native scheduler publication is the acknowledgement; elapsed time is
    // only a watchdog, never evidence that a capture has finished.
    {
        let (observed, _) = state
            .changed
            .wait_timeout_while(state.lock(), Duration::from_secs(12), |data| {
                !data
                    .jobs
                    .iter()
                    .any(|job| job.id == id && !job.status.active())
            })
            .unwrap();
        assert!(
            observed
                .jobs
                .iter()
                .any(|job| job.id == id && job.status == Status::Completed)
        );
        assert_eq!(observed.jobs.iter().filter(|job| job.id == id).count(), 1);
        assert!(observed.viewers.is_empty());
    }
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
    request.end = request.start + 60;
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
    // Native startup and filesystem work can exceed a three-second window on
    // Windows CI. Observe actual appended media, then stop this one capture;
    // neither a fixed sleep nor wall-clock setup speed is the behavior here.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::fs::metadata(&spool).is_ok_and(|meta| meta.len() / 188 <= 1) {
        assert!(
            std::time::Instant::now() < deadline,
            "capture did not append media"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let stop = state.lock().active.get(&id).map(|(_, token)| token.clone());
    if let Some(stop) = stop {
        stop.cancel();
    }
    let job = loop {
        let job = services
            .read(move |store| store.recording_job(id))
            .await
            .unwrap()
            .unwrap();
        if !job.status.active() {
            break job;
        }
        assert!(std::time::Instant::now() < deadline, "capture did not stop");
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
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
async fn accepting_a_recording_needs_the_background_host_before_it_returns() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory.path().join("generated-app"));
    let request = seed(&owner, "https://example.test/generated.ts").await;
    assert!(!owner.recording_background_needed());
    let id = owner.schedule_recording(request).await.unwrap();
    assert!(owner.recording_background_needed());
    assert!(
        owner
            .recording_snapshot()
            .iter()
            .any(|job| job.id == id && job.status == Status::Scheduled)
    );
    owner.stop_recording(id).await.unwrap();
    assert!(owner.drain_services());
}

#[derive(Default)]
struct PublicationGate {
    entered: AtomicBool,
    released: Mutex<bool>,
    changed: Condvar,
}
static PUBLICATION_GATE: Mutex<Option<Arc<PublicationGate>>> = Mutex::new(None);
fn blocking_publication(active: bool) {
    if !active {
        return;
    }
    let gate = PUBLICATION_GATE.lock().unwrap().clone();
    if let Some(gate) = gate {
        gate.entered.store(true, Ordering::SeqCst);
        let released = gate.released.lock().unwrap();
        let _ = gate
            .changed
            .wait_timeout_while(released, Duration::from_secs(10), |released| !*released)
            .unwrap();
    }
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
async fn an_accepted_capture_waits_for_the_host_without_blocking_the_ui_executor() {
    let directory = tempfile::tempdir().unwrap();
    let gate = Arc::new(PublicationGate::default());
    *PUBLICATION_GATE.lock().unwrap() = Some(gate.clone());
    let release = ReleasePublication(gate.clone());
    let mut ports = crate::Ports::new(directory.path().join("generated-app"), |_| {
        Ok(iptv_store_sqlite::DatabaseKey::from_bytes([17; 32]))
    });
    ports.recording_schedule = blocking_publication;
    let owner = Runtime::new(ports);
    owner.start_services();
    let mut request = seed(&owner, "https://example.test/generated.ts").await;
    // Record now: the host is told a capture is due.
    request.start = now();
    let mut accepted = Box::pin(owner.schedule_recording(request));
    tokio::select! {
        result=&mut accepted=>panic!("capture acknowledged before the host applied it: {result:?}"),
        _=async {while !gate.entered.load(Ordering::SeqCst) {tokio::time::sleep(Duration::from_millis(10)).await;}}=>{},
        _=tokio::time::sleep(Duration::from_secs(3))=>panic!("the host was not told"),
    }
    assert_eq!(
        owner
            .services()
            .read(|store| store.recording_jobs())
            .await
            .unwrap()
            .len(),
        1,
        "intent is durable while the host call is still blocked"
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
    assert!(owner.drain_services());
}

async fn protect_recording(owner: &Runtime, request: &Request) -> iptv_core::parental::Target {
    use iptv_core::parental::{Pin, Target};
    let mut settings = (*owner.settings()).clone();
    settings
        .set("parental.live_enabled", iptv_settings::Value::Bool(true))
        .unwrap();
    *owner.services().shared_settings().write().unwrap() = Some(Arc::new(settings));
    owner
        .parental_change_pin(None, Pin::parse("2468").unwrap())
        .await
        .unwrap();
    let target = Target::live(request.source, request.channel.as_str());
    let source = SourceId(request.source);
    let channel = ContentId::from_stored(request.channel.clone());
    owner
        .services()
        .call(move |store| store.set_blocked(source, &channel, true))
        .await
        .unwrap();
    owner
        .parental_authorize(target.clone(), Pin::parse("2468").unwrap())
        .await
        .unwrap();
    target
}

#[tokio::test(flavor = "current_thread")]
async fn a_queued_recording_rechecks_the_grant_in_the_accepting_writer() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(directory.path());
    let request = seed(&owner, "https://example.test/generated.ts").await;
    let initial = request.clone();
    let services = owner.services();
    services
        .call(move |store| {
            store.schedule_recording(JobId(951), &initial, now())?;
            Ok(())
        })
        .await
        .unwrap();
    let target = protect_recording(&owner, &request).await;
    let admission = owner.recording_admission(&request).await.unwrap();
    let (entered, arrived) = std::sync::mpsc::channel();
    let (release, held) = std::sync::mpsc::channel();
    let writer = services.call(move |_| {
        entered.send(()).unwrap();
        held.recv_timeout(Duration::from_secs(5)).unwrap();
        Ok(())
    });
    arrived.recv_timeout(Duration::from_secs(2)).unwrap();
    let accepted = admission.clone();
    let duplicate = request.clone();
    let scheduling = services.call(move |store| {
        store.schedule_recording_checked(JobId(954), &duplicate, now(), |store| {
            accepted.authorize(store)
        })
    });
    owner.parental_lock();
    release.send(()).unwrap();
    writer.await.unwrap();
    assert!(scheduling.await.is_err());
    services
        .read(move |store| {
            assert_eq!(store.recording_jobs()?.len(), 1);
            Ok(())
        })
        .await
        .unwrap();
    // A fresh unlock admits the legacy blocked channel; the raw store still cannot.
    owner
        .parental_authorize(target, iptv_core::parental::Pin::parse("2468").unwrap())
        .await
        .unwrap();
    let admission = owner.recording_admission(&request).await.unwrap();
    assert_eq!(
        services
            .call(move |store| store.schedule_recording_checked(
                JobId(957),
                &request,
                now(),
                |store| admission.authorize(store)
            ))
            .await
            .unwrap(),
        JobId(951)
    );
    assert!(owner.drain_services());
}

#[tokio::test(flavor = "current_thread")]
async fn recording_admission_cannot_follow_a_reused_numeric_source() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(directory.path());
    let request = seed(&owner, "https://example.test/generated.ts").await;
    let admission = owner.recording_admission(&request).await.unwrap();
    let source = SourceId(request.source);
    owner.services().call(move |store| {
        let config = store.source(source)?.unwrap().source;
        store.delete_source(source)?;
        assert_eq!(store.add_source(&config)?, source);
        let parsed = iptv_providers::m3u::parse_m3u("#EXTM3U\n#EXTINF:-1 tvg-id=\"generated\",Generated channel\nhttps://example.test/generated.ts\n");
        store.replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
    }).await.unwrap();
    assert!(
        owner
            .services()
            .call(move |store| store.schedule_recording_checked(
                JobId(961),
                &request,
                now(),
                |store| admission.authorize(store)
            ))
            .await
            .is_err()
    );
    assert!(
        owner
            .services()
            .read(|store| store.recording_jobs())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(owner.drain_services());
}

#[tokio::test(flavor = "current_thread")]
async fn recording_file_and_relay_reject_relock_after_their_last_service_await() {
    for stage in ["file", "relay"] {
        let directory = tempfile::tempdir().unwrap();
        let owner = owner(directory.path());
        let request = seed(&owner, "https://example.test/generated.ts").await;
        let id = JobId(971);
        let staged = request.clone();
        owner
            .services()
            .call(move |store| {
                store.schedule_recording(id, &staged, now())?;
                assert!(store.transition_recording(
                    id,
                    Status::Scheduled,
                    Status::Completed,
                    188,
                    "Synthetic retained media"
                )?);
                Ok(())
            })
            .await
            .unwrap();
        std::fs::create_dir_all(media_directory(owner.data_dir())).unwrap();
        std::fs::write(path(&owner.services(), id, false), [0x47; 188]).unwrap();
        protect_recording(&owner, &request).await;
        let (arrived, waiting) = tokio::sync::oneshot::channel();
        let (release, held) = tokio::sync::oneshot::channel();
        *owner.recordings.playback_completion.lock().unwrap() = Some((stage, arrived, held));
        let opening = async {
            if stage == "file" {
                owner.recording_file(id).await.map(drop)
            } else {
                owner.open_recording(id).await.map(drop)
            }
        };
        tokio::pin!(opening);
        tokio::select! {
            result = &mut opening => panic!("{stage} passed the completion hold: {result:?}"),
            arrived = tokio::time::timeout(Duration::from_secs(5), waiting) => arrived.unwrap().unwrap(),
        }
        owner.parental_lock();
        release.send(()).unwrap();
        assert!(
            opening.await.is_err(),
            "{stage} must not return a revoked capability"
        );
        assert!(
            owner.recordings.playback.lock().unwrap().is_none(),
            "an unaccepted relay must not acquire the playback lease"
        );
        assert!(owner.drain_services());
    }
}

async fn capture_accepted_media(owner: &Runtime, id: JobId) {
    until(owner, id, |job| job.status == Status::Recording).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !std::fs::metadata(path(&owner.services(), id, true)).is_ok_and(|m| m.len() >= 188) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("accepted capture writes actual transport packets");
    owner.stop_recording(id).await.unwrap();
    let retained = until(owner, id, |job| !job.status.active()).await;
    assert_eq!(retained.status, Status::Completed, "{retained:?}");
    assert!(retained.bytes >= 188);
    assert!(
        std::fs::read(path(&owner.services(), id, false))
            .unwrap()
            .starts_with(&[0x47])
    );
    // Retained recording authority is distinct from acquiring a playback grant.
    assert!(owner.recording_file(id).await.is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn an_unlocked_legacy_blocked_recording_captures_after_the_viewer_relocks() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(directory.path());
    let mut request = seed(&owner, &server()).await;
    let target = protect_recording(&owner, &request).await;
    request.start = now();
    request.end = now() + 60;
    let id = owner.schedule_recording(request.clone()).await.unwrap();
    owner.parental_lock();
    assert!(owner.parental_check(&target).await.is_err());
    assert!(
        owner.schedule_recording(request).await.is_err(),
        "new requests still need a current unlock"
    );
    capture_accepted_media(&owner, id).await;
    assert!(owner.drain_services());
}

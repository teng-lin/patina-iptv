//! A parked capture keeps its bytes even when its remaining window cannot run.
use super::*;
use iptv_providers::{SecretUrl, model::M3uSourceConfig};

pub(super) async fn fixture(root: &Path) -> (Runtime, Request) {
    let owner = Runtime::new(crate::Ports::new(root.to_owned(), |_| {
        Ok(crate::model::DatabaseKey::from_bytes([0x75; 32]))
    }));
    owner.start_services();
    let request = owner
        .services()
        .call(|store| {
            let source = store.add_source(&iptv_providers::model::Source {
                name: "Generated recovery source".into(),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: SecretUrl::new("https://example.test/generated.m3u"),
                    user_agent: None,
                    referrer: None,
                }),
            })?;
            let parsed = iptv_providers::m3u::parse_m3u(
                "#EXTM3U\n#EXTINF:-1,Generated channel\nhttps://example.test/generated.ts\n",
            );
            let channel = parsed.channels.items[0].id.to_string();
            store.replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])?;
            Ok(Request {
                source: source.0,
                channel,
                channel_name: "Generated channel".into(),
                title: "Generated retained program".into(),
                start: now() - 120,
                end: now() - 60,
            })
        })
        .await
        .unwrap();
    (owner, request)
}

async fn park(owner: &Runtime, request: Request) -> JobId {
    let id = JobId(751);
    std::fs::create_dir_all(media_directory(owner.data_dir())).unwrap();
    std::fs::write(path(&owner.services(), id, true), [0x47; 188]).unwrap();
    owner
        .services()
        .call(move |store| {
            // The scheduler has a separate read lane. Keep this synthetic job
            // in the future until both its spool and retained-byte row exist;
            // otherwise it can correctly miss the half-prepared zero-byte job.
            let staged_at = now();
            let staged = Request {
                start: staged_at + 3600,
                end: staged_at + 3660,
                ..request.clone()
            };
            store.schedule_recording(id, &staged, staged_at)?;
            assert!(store.transition_recording(
                id,
                Status::Scheduled,
                Status::Scheduled,
                188,
                "Resuming interrupted recording",
            )?);
            assert!(store.update_recording(id, &request, request.start - 60)?);
            Ok(())
        })
        .await
        .unwrap();
    id
}

async fn ready(owner: &Runtime) {
    owner.start_recordings();
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while !owner.recordings.lock().ready {
        assert!(std::time::Instant::now() < until, "recorder did not start");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn assert_retained(owner: &Runtime, id: JobId) {
    let until = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let job = owner
            .recording_jobs()
            .await
            .unwrap()
            .into_iter()
            .find(|job| job.id == id)
            .unwrap();
        if job.status == Status::Partial {
            assert_eq!(job.bytes, 188);
            let file = owner.recording_file(id).await.unwrap();
            assert_eq!(std::fs::read(file).unwrap(), [0x47; 188]);
            assert!(!path(&owner.services(), id, true).exists());
            return;
        }
        assert!(
            job.status == Status::Scheduled && std::time::Instant::now() < until,
            "captured bytes must stay playable: {job:?}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn startup_retains_parked_media_after_the_recording_window_elapsed() {
    let directory = tempfile::tempdir().unwrap();
    let (owner, request) = fixture(&directory.path().join("profile")).await;
    let id = park(&owner, request).await;
    recover(&owner.services()).await.unwrap();
    assert_retained(&owner, id).await;
    assert!(owner.drain_services());
}

#[tokio::test]
async fn running_host_retains_parked_media_after_a_missed_window() {
    let directory = tempfile::tempdir().unwrap();
    let (owner, request) = fixture(&directory.path().join("profile")).await;
    ready(&owner).await;
    let id = park(&owner, request).await;
    assert_retained(&owner, id).await;
    assert!(owner.drain_services());
}

#[tokio::test]
async fn running_host_retains_parked_media_when_viewing_uses_the_only_connection() {
    let directory = tempfile::tempdir().unwrap();
    let (owner, mut request) = fixture(&directory.path().join("profile")).await;
    assert!(owner.reserve_viewing(0, SourceId(request.source)));
    ready(&owner).await;
    request.start = now() - 1;
    request.end = now() + 60;
    let id = park(&owner, request).await;
    assert_retained(&owner, id).await;
    assert!(owner.drain_services());
}

/// The upgrade that retired scheduled recording turns a capture interrupted
/// mid-window into a partial row whose size was never stored. Recovery
/// finalizes its spool and records the size, so it plays, and never resumes
/// the capture.
#[tokio::test]
async fn startup_finalizes_a_capture_the_upgrade_marked_partial() {
    let directory = tempfile::tempdir().unwrap();
    let (owner, mut request) = fixture(&directory.path().join("profile")).await;
    request.start = now() - 60;
    request.end = now() + 3600;
    let id = JobId(752);
    std::fs::create_dir_all(media_directory(owner.data_dir())).unwrap();
    std::fs::write(path(&owner.services(), id, true), [0x47; 188]).unwrap();
    owner
        .services()
        .call(move |store| {
            store.schedule_recording(id, &request, request.start)?;
            assert!(store.transition_recording(
                id,
                Status::Scheduled,
                Status::Partial,
                0,
                "Interrupted when the app closed",
            )?);
            Ok(())
        })
        .await
        .unwrap();
    recover(&owner.services()).await.unwrap();
    let job = owner
        .services()
        .read(move |store| store.recording_job(id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!((job.status, job.bytes), (Status::Partial, 188));
    let file = owner.recording_file(id).await.unwrap();
    assert_eq!(std::fs::read(file).unwrap(), [0x47; 188]);
    assert!(!path(&owner.services(), id, true).exists());
    assert!(owner.drain_services());
}

#[test]
fn quota_reserves_remaining_bytes_and_accepts_four_growing_captures() {
    let directory = tempfile::tempdir().unwrap();
    let active = [JobId(1), JobId(2), JobId(3), JobId(4)];
    for job in active {
        let file = std::fs::File::create(directory.path().join(format!("{}.part", job.0))).unwrap();
        file.set_len(188).unwrap();
    }
    assert!(storage_admission(directory.path(), &active).is_ok());
    // An unrelated retained recording consumes capacity beyond the four reservations.
    std::fs::write(directory.path().join("5.ts"), [0x47; 188]).unwrap();
    assert_eq!(
        storage_admission(directory.path(), &active)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::StorageFull
    );
    // Removing one active reservation leaves room for retained bytes.
    assert!(storage_admission(directory.path(), &active[..3]).is_ok());
}

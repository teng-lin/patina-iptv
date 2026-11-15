//! A parked capture keeps its bytes even when its remaining window cannot run.
use super::*;
use iptv_providers::{SecretUrl, model::M3uSourceConfig};

async fn fixture(root: &Path) -> (Runtime, Request) {
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
                title: "Generated retained programme".into(),
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
            store.schedule_recording(id, &request, request.start - 60)?;
            store.transition_recording(
                id,
                Status::Scheduled,
                Status::Scheduled,
                188,
                "Resuming interrupted recording",
            )?;
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
    let (owner, request) = fixture(directory.path()).await;
    let id = park(&owner, request).await;
    recover(&owner.services()).await.unwrap();
    assert_retained(&owner, id).await;
    assert!(owner.drain_services());
}

#[tokio::test]
async fn running_host_retains_parked_media_after_a_missed_window() {
    let directory = tempfile::tempdir().unwrap();
    let (owner, request) = fixture(directory.path()).await;
    ready(&owner).await;
    let id = park(&owner, request).await;
    assert_retained(&owner, id).await;
    assert!(owner.drain_services());
}

#[tokio::test]
async fn running_host_retains_parked_media_when_viewing_uses_the_only_connection() {
    let directory = tempfile::tempdir().unwrap();
    let (owner, mut request) = fixture(directory.path()).await;
    assert!(owner.reserve_viewing(0, SourceId(request.source)));
    ready(&owner).await;
    request.start = now() - 1;
    request.end = now() + 60;
    let id = park(&owner, request).await;
    assert_retained(&owner, id).await;
    assert!(owner.drain_services());
}

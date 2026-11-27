use super::*;
use crate::app_updates::{InstalledPackage, UpdateResult};
use serde_json::json;

#[test]
fn file_commit_admission_tracks_pending_workers_and_is_owned_by_each_runtime() {
    use std::{
        cell::Cell,
        future::Future,
        task::{Context, Poll, Waker},
    };

    let owner = crate::Runtime::default();
    let other = crate::Runtime::default();
    assert!(!owner.app_update_file_commit_active());
    for outcome in [false, true] {
        let result = Cell::new(None);
        let mut worker = Box::pin(async {
            let _commit = owner.begin_app_update_file_commit();
            std::future::poll_fn(|_| result.get().map_or(Poll::Pending, Poll::Ready)).await
        });
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(worker.as_mut().poll(&mut context), Poll::Pending);
        assert!(owner.app_update_file_commit_active());
        assert!(!other.app_update_file_commit_active());
        // A separate shell's operation must neither share nor clear admission.
        let separate = other.begin_app_update_file_commit();
        let nested = owner.begin_app_update_file_commit();
        drop(nested);
        assert!(owner.app_update_file_commit_active());
        result.set(Some(outcome));
        assert_eq!(worker.as_mut().poll(&mut context), Poll::Ready(outcome));
        assert!(!owner.app_update_file_commit_active());
        assert!(other.app_update_file_commit_active());
        drop(separate);
        assert!(!other.app_update_file_commit_active());
    }

    let mut worker = Box::pin(async {
        let _commit = owner.begin_app_update_file_commit();
        std::future::pending::<()>().await;
    });
    assert_eq!(
        worker
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    );
    assert!(owner.app_update_file_commit_active());
    drop(worker); // Disposing the shell task releases its borrowed admission.
    assert!(!owner.app_update_file_commit_active());
    assert!(!other.app_update_file_commit_active());
}

fn asset() -> ReleaseAsset {
    let installed = InstalledPackage {
        platform: "android",
        architecture: "arm64-v8a",
        format: Some("apk".into()),
        version: "9.8.6",
        android_code: Some(10_008_006),
        minimum: json!({"androidApi": 30}),
    };
    let Some(UpdateResult::Available { asset, .. }) = super::super::selection::select(
        include_bytes!("../../../../tests/fixtures/updates/index.json"),
        installed,
    ) else {
        panic!("expected synthetic selected package");
    };
    asset
}

fn download(directory: &std::path::Path) -> TempPath {
    let file = tempfile::NamedTempFile::new_in(directory).unwrap();
    std::fs::write(file.path(), b"synthetic").unwrap();
    file.into_temp_path()
}

#[test]
fn about_reopen_reclaims_only_the_exact_asset_without_deleting_verified_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let owner = crate::Runtime::default();
    let asset = asset();
    let stage = download(temp.path());
    let location = stage.to_path_buf();
    owner.retain_update_download(asset.clone(), stage);
    assert!(owner.has_pending_app_update());
    assert_eq!(std::fs::read(&location).unwrap(), b"synthetic");
    for different in [
        |asset: &mut ReleaseAsset| asset.url.push_str(".different"),
        |asset: &mut ReleaseAsset| asset.sha256 = "0".repeat(64),
        |asset: &mut ReleaseAsset| asset.length += 1,
    ] {
        let mut mismatch = asset.clone();
        different(&mut mismatch);
        assert!(owner.take_update_download(&mismatch).is_none());
        assert!(owner.has_pending_app_update());
        assert!(location.exists());
    }
    let reused = owner.take_update_download(&asset).unwrap();
    assert_eq!(reused.to_path_buf(), location);
    assert_eq!(std::fs::read(&reused).unwrap(), b"synthetic");
    assert!(!owner.has_pending_app_update());
    assert!(owner.take_update_download(&asset).is_none());
    drop(reused);
    assert!(!location.exists());
}

#[test]
fn retained_download_is_bounded_and_cancel_or_runtime_shutdown_cleans_it() {
    let temp = tempfile::tempdir().unwrap();
    let owner = crate::Runtime::default();
    let asset = asset();
    let first = download(temp.path());
    let previous = first.to_path_buf();
    owner.retain_update_download(asset.clone(), first);
    let replacement = download(temp.path());
    let location = replacement.to_path_buf();
    owner.retain_update_download(asset.clone(), replacement);
    assert!(!previous.exists());
    assert!(location.exists());
    owner.cancel_pending_app_update();
    assert!(!owner.has_pending_app_update());
    assert!(!location.exists());
    let last = download(temp.path());
    let location = last.to_path_buf();
    owner.retain_update_download(asset, last);
    drop(owner);
    assert!(!location.exists());
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn cancellation_and_stale_completion_cannot_clear_a_new_downloads_progress() {
    let owner = crate::Runtime::default();
    let (old, old_progress, old_cancel) = owner.begin_update_download(9);
    assert_eq!(owner.app_update_download_progress(), Some((0, 9)));
    old_progress.received.store(3, Ordering::Relaxed);
    assert_eq!(owner.app_update_download_progress(), Some((3, 9)));
    let (new, _, new_cancel) = owner.begin_update_download(12);
    assert!(old_cancel.await.is_err());
    assert!(!old.is_current());
    assert!(new.is_current());
    drop(old);
    assert_eq!(owner.app_update_download_progress(), Some((0, 12)));
    owner.cancel_app_update_download();
    assert!(new_cancel.await.is_err());
    assert!(!new.is_current());
    assert!(owner.app_update_download_progress().is_none());
    drop(new);
    assert!(owner.app_update_download_progress().is_none());
}

#[cfg(all(feature = "test-support", debug_assertions))]
#[test]
fn actual_streaming_download_reports_progress_and_cancel_removes_the_partial_file() {
    const CHILD: &str = "IPTV_TEST_UPDATE_PROGRESS_CHILD";
    const SIGNAL: &str = "IPTV_TEST_UPDATE_PROGRESS_SIGNAL";
    if std::env::var_os(CHILD).is_some() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(streaming_journey());
        return;
    }
    use std::{
        io::{Read, Write},
        net::TcpListener,
        time::{Duration, Instant},
    };
    let directory = tempfile::tempdir().unwrap();
    let signal = directory.path().join("continue");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/download", listener.local_addr().unwrap());
    let observed = signal.clone();
    let server = std::thread::spawn(move || {
        for complete in [true, false] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let length = stream.read(&mut buffer).unwrap();
                assert!(length > 0);
                request.extend_from_slice(&buffer[..length]);
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\nsyn")
                .unwrap();
            stream.flush().unwrap();
            if complete {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !observed.exists() {
                    assert!(Instant::now() < deadline, "download progress not observed");
                    std::thread::sleep(Duration::from_millis(1));
                }
                stream.write_all(b"thetic").unwrap();
            } else {
                // Cancellation closes the partial HTTP response without waiting
                // for its remaining bytes or permitting an installer handoff.
                match stream.read(&mut buffer) {
                    Ok(0) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
                    result => panic!("cancellation must close the response socket: {result:?}"),
                }
            }
        }
    });
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "native::app_updates::pending::tests::actual_streaming_download_reports_progress_and_cancel_removes_the_partial_file", "--nocapture"])
        .env(CHILD, "1").env("IPTV_TEST_UPDATE_DOWNLOAD_API", endpoint).env(SIGNAL, &signal)
        .output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    server.join().unwrap();
}

#[cfg(all(feature = "test-support", debug_assertions))]
async fn streaming_journey() {
    use std::time::Duration;
    let directory = tempfile::tempdir().unwrap();
    let owner = crate::Runtime::default();
    owner.start_services();
    for complete in [true, false] {
        let operation = owner.download_update(asset(), directory.path().into());
        tokio::pin!(operation);
        let observe = async {
            loop {
                if owner.app_update_download_progress() == Some((3, 9)) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        };
        tokio::select! {
            result = &mut operation => panic!("download completed before controlled response: {result:?}"),
            result = tokio::time::timeout(Duration::from_secs(5), observe) => result.unwrap(),
        }
        assert_eq!(owner.app_update_download_progress(), Some((3, 9)));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        if complete {
            std::fs::write(
                std::env::var_os("IPTV_TEST_UPDATE_PROGRESS_SIGNAL").unwrap(),
                b"continue",
            )
            .unwrap();
            let path = operation.await.unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), b"synthetic");
            assert!(owner.app_update_download_progress().is_none());
            owner.retain_update_download(asset(), path);
            assert!(owner.has_pending_app_update());
            let reused = owner.take_update_download(&asset()).unwrap();
            assert_eq!(std::fs::read(&reused).unwrap(), b"synthetic");
            drop(reused);
        } else {
            owner.cancel_app_update_download();
            assert!(operation.await.is_none());
            assert!(owner.app_update_download_progress().is_none());
            assert!(!owner.has_pending_app_update());
        }
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}

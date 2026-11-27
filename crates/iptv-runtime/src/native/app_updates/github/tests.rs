use super::*;
use serde_json::json;
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread::{self, JoinHandle},
    time::Instant,
};

fn installed(version: &'static str) -> InstalledPackage {
    InstalledPackage {
        platform: "linux",
        architecture: "x86_64",
        format: Some("appimage".into()),
        version,
        android_code: None,
        minimum: json!({}),
    }
}

fn release(version: &str) -> Value {
    let name = format!("Patina-{version}-linux-x86_64.AppImage");
    json!({
        "tag_name": format!("v{version}"), "draft": false, "prerelease": false,
        "html_url": format!("{RELEASES}/tag/v{version}"),
        "body": "## Synthetic release\n\nRepair playback and preserve the library.",
        "published_at": "2026-11-27T10:00:00Z",
        "assets": [{"name": name, "state": "uploaded", "size": 512,
            "browser_download_url": format!("{RELEASES}/download/v{version}/{name}")}]
    })
}

fn serve(responses: Vec<(u16, Vec<u8>, Duration)>) -> (String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/releases", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let worker = thread::spawn(move || {
        let mut requests = Vec::new();
        for (status, body, delay) in responses {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "fixture received no request");
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let length = socket.read(&mut buffer).unwrap();
                assert!(length > 0);
                request.extend_from_slice(&buffer[..length]);
            }
            requests.push(String::from_utf8(request).unwrap());
            thread::sleep(delay);
            let header = format!(
                "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/json\r\n\r\n",
                body.len()
            );
            if socket.write_all(header.as_bytes()).is_ok() {
                let _ = socket.write_all(&body);
            }
        }
        requests
    });
    (endpoint, worker)
}

fn fixture(body: Value) -> (String, JoinHandle<Vec<String>>) {
    serve(vec![(
        200,
        serde_json::to_vec(&body).unwrap(),
        Duration::ZERO,
    )])
}

async fn received(worker: JoinHandle<Vec<String>>) -> Vec<String> {
    // Let the current-thread runtime finish closing a rejected response while
    // waiting for a fixture that may still be writing that response's body.
    tokio::task::spawn_blocking(move || worker.join().unwrap())
        .await
        .unwrap()
}

async fn get(endpoint: &str, installed: InstalledPackage) -> Result<ReleaseCheck, CheckError> {
    let client = NetClient::new(config()).unwrap();
    check(&client, endpoint, false, installed, DEADLINE).await
}

#[tokio::test]
async fn fetches_highest_semantic_stable_release_and_inline_notes_without_provider_headers() {
    let mut draft = release("99.0.0");
    draft["draft"] = json!(true);
    let mut preview = release("10.0.0");
    preview["prerelease"] = json!(true);
    let (endpoint, worker) = fixture(json!([
        release("0.1.9"),
        release("0.1.10"),
        release("0.1.8"),
        draft,
        preview,
        release("12.0.0-beta.1")
    ]));
    let result = get(&endpoint, installed("0.1.9")).await.unwrap();
    assert_eq!(result.status, ReleaseStatus::UpdateAvailable);
    assert_eq!(result.release.version, "0.1.10");
    assert!(result.release.notes.contains("Repair playback"));
    assert_eq!(
        result.release.published_at.as_deref(),
        Some("2026-11-27T10:00:00Z")
    );
    assert_eq!(
        result.release.download.unwrap().name,
        "Patina-0.1.10-linux-x86_64.AppImage"
    );
    let requests = received(worker).await;
    let request = requests[0].to_ascii_lowercase();
    assert!(request.starts_with("get /releases?per_page=100&page=1 "));
    assert!(request.contains("accept: application/vnd.github+json"));
    assert!(request.contains("x-github-api-version: 2022-11-28"));
    assert!(request.contains("user-agent: patina"));
    for name in ["authorization:", "cookie:", "referer:"] {
        assert!(!request.contains(name));
    }
}

#[tokio::test]
async fn never_downgrades_and_ignores_build_metadata_for_precedence() {
    for (version, expected) in [
        ("0.1.10", ReleaseStatus::Current),
        ("v0.1.10+synthetic.2", ReleaseStatus::Current),
        ("0.1.11-dev.1", ReleaseStatus::Ahead),
        ("0.1.10-beta.1", ReleaseStatus::UpdateAvailable),
    ] {
        let (endpoint, worker) = fixture(json!([release("0.1.10")]));
        assert_eq!(
            get(&endpoint, installed(version)).await.unwrap().status,
            expected
        );
        received(worker).await;
    }
    assert_eq!(
        get("http://127.0.0.1:1", installed("not-a-version")).await,
        Err(CheckError::InvalidInstalledVersion)
    );
}

#[tokio::test]
async fn newer_release_without_matching_package_still_supplies_notes_and_release_link() {
    for (platform, architecture, format) in [
        ("linux", "aarch64", Some("appimage")),
        ("linux", "x86_64", None),
        ("linux", "x86_64", Some("deb")),
        ("ios", "aarch64", None),
    ] {
        let mut package = installed("0.1.9");
        package.platform = platform;
        package.architecture = architecture;
        package.format = format.map(str::to_owned);
        let (endpoint, worker) = fixture(json!([release("0.1.10")]));
        let result = get(&endpoint, package).await.unwrap();
        assert_eq!(result.status, ReleaseStatus::NoCompatiblePackage);
        assert_eq!(result.release.url, format!("{RELEASES}/tag/v0.1.10"));
        assert!(result.release.download.is_none());
        assert!(!result.release.notes.is_empty());
        received(worker).await;
    }
}

#[tokio::test]
async fn failed_package_upload_keeps_notes_available_and_does_not_shadow_an_uploaded_package() {
    for uploaded in [false, true] {
        let mut data = release("0.1.10");
        let mut starter = data["assets"][0].clone();
        starter["state"] = json!("starter");
        starter["size"] = json!(0);
        if !uploaded {
            data["assets"] = json!([]);
        }
        data["assets"].as_array_mut().unwrap().insert(0, starter);
        let (endpoint, worker) = fixture(json!([data]));
        let checked = get(&endpoint, installed("0.1.9")).await.unwrap();
        assert_eq!(checked.release.version, "0.1.10");
        assert_eq!(checked.release.url, format!("{RELEASES}/tag/v0.1.10"));
        assert!(checked.release.notes.contains("Repair playback"));
        if uploaded {
            assert_eq!(checked.status, ReleaseStatus::UpdateAvailable);
            assert_eq!(
                checked.release.download.unwrap().name,
                "Patina-0.1.10-linux-x86_64.AppImage"
            );
        } else {
            assert_eq!(checked.status, ReleaseStatus::NoCompatiblePackage);
            assert!(checked.release.download.is_none());
        }
        received(worker).await;
    }
}

#[test]
fn package_matching_respects_architecture_and_installation_channel() {
    for (platform, arch, format, expected) in [
        ("android", "arm64-v8a", None, None),
        ("android", "armeabi-v7a", None, None),
        (
            "android",
            "arm64-v8a",
            Some("apk"),
            Some("android-tablet-tv.apk"),
        ),
        (
            "android",
            "armeabi-v7a",
            Some("apk"),
            Some("android-tablet-tv.apk"),
        ),
        ("android", "x86_64", None, None),
        ("android", "arm64-v8a", Some("store"), None),
        ("android", "armeabi-v7a", Some("store"), None),
        ("macos", "arm64", None, Some("macos-arm64.dmg")),
        ("macos", "x86_64", None, None),
        ("windows", "x86_64", None, Some("windows-x86_64-setup.exe")),
        ("windows", "aarch64", None, None),
        ("linux", "x86_64", Some("rpm"), Some("linux-x86_64.rpm")),
        (
            "linux",
            "x86_64",
            Some("pacman"),
            Some("linux-x86_64.pkg.tar.zst"),
        ),
    ] {
        let mut package = installed("0.1.9");
        package.platform = platform;
        package.architecture = arch;
        package.format = format.map(str::to_owned);
        assert_eq!(suffix(&package), expected, "{platform} {arch} {format:?}");
    }
}

#[tokio::test]
async fn android_release_discovery_offers_apk_only_to_explicit_direct_installations() {
    let mut data = release("0.1.10");
    let name = "Patina-0.1.10-android-tablet-tv.apk";
    data["assets"] = json!([{
        "name": name, "size": 9,
        "browser_download_url": format!("{RELEASES}/download/v0.1.10/{name}")
    }]);
    for architecture in ["arm64-v8a", "armeabi-v7a"] {
        for format in [None, Some("store"), Some("apk")] {
            let mut package = installed("0.1.9");
            package.platform = "android";
            package.architecture = architecture;
            package.format = format.map(str::to_owned);
            let (endpoint, worker) = fixture(json!([data]));
            let checked = get(&endpoint, package).await.unwrap();
            assert_eq!(checked.release.version, "0.1.10");
            assert!(!checked.release.notes.is_empty());
            if format == Some("apk") {
                assert_eq!(checked.status, ReleaseStatus::UpdateAvailable);
                assert_eq!(checked.release.download.unwrap().name, name);
            } else {
                assert_eq!(checked.status, ReleaseStatus::NoCompatiblePackage);
                assert!(checked.release.download.is_none());
            }
            received(worker).await;
        }
    }
}

#[cfg(all(feature = "test-support", debug_assertions))]
#[test]
fn fixture_override_accepts_only_literal_loopback_http_without_credentials() {
    for value in [
        "http://127.0.0.1:1234/releases",
        "http://[::1]:1234/releases",
    ] {
        assert!(fixture_endpoint(value), "{value}");
    }
    for value in [
        "https://127.0.0.1/releases",
        "http://localhost/releases",
        "http://192.0.2.1/releases",
        "http://127.0.0.1.example.invalid/releases",
        "http://user:secret@127.0.0.1/releases",
        "http://127.0.0.1/releases?token=synthetic",
        "http://127.0.0.1/releases#fragment",
        "file:///synthetic.json",
    ] {
        assert!(!fixture_endpoint(value), "{value}");
    }
}

#[tokio::test]
async fn paginates_instead_of_treating_first_page_as_highest_version() {
    let first = vec![release("0.1.9"); PAGE_SIZE];
    let (endpoint, worker) = serve(vec![
        (200, serde_json::to_vec(&first).unwrap(), Duration::ZERO),
        (
            200,
            serde_json::to_vec(&json!([release("0.2.0")])).unwrap(),
            Duration::ZERO,
        ),
    ]);
    assert_eq!(
        get(&endpoint, installed("0.1.9"))
            .await
            .unwrap()
            .release
            .version,
        "0.2.0"
    );
    let requests = received(worker).await;
    assert!(requests[1].starts_with("GET /releases?per_page=100&page=2 "));
}

#[tokio::test]
async fn reports_network_rate_limit_timeout_and_invalid_responses_as_errors() {
    for (status, body, expected) in [
        (403, b"rate limited".to_vec(), CheckError::RateLimited),
        (429, b"rate limited".to_vec(), CheckError::RateLimited),
        (500, b"server failure".to_vec(), CheckError::Http(500)),
        (200, b"not JSON".to_vec(), CheckError::InvalidResponse),
        (200, b"{}".to_vec(), CheckError::InvalidResponse),
        (200, b"[]".to_vec(), CheckError::NoStableRelease),
        (200, vec![b' '; MAX_BODY + 1], CheckError::InvalidResponse),
    ] {
        let (endpoint, worker) = serve(vec![(status, body, Duration::ZERO)]);
        assert_eq!(get(&endpoint, installed("0.1.9")).await, Err(expected));
        received(worker).await;
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/releases", listener.local_addr().unwrap());
    drop(listener);
    assert_eq!(
        get(&endpoint, installed("0.1.9")).await,
        Err(CheckError::Network)
    );

    for deadline in [Duration::ZERO, Duration::from_millis(20)] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/releases", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let (stop, stopped) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || {
            let mut sockets = Vec::new();
            while stopped.try_recv() == Err(std::sync::mpsc::TryRecvError::Empty) {
                match listener.accept() {
                    Ok((socket, _)) => sockets.push(socket),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("{error}"),
                }
            }
            // Keep accepted connections open without replying. The deadline
            // may cancel before a connection or complete headers ever arrive.
        });
        let client = NetClient::new(config()).unwrap();
        let result = check(&client, &endpoint, false, installed("0.1.9"), deadline).await;
        stop.send(()).unwrap();
        tokio::task::spawn_blocking(move || worker.join().unwrap())
            .await
            .unwrap();
        assert_eq!(result, Err(CheckError::Timeout));
    }
}

#[tokio::test]
async fn rejects_untrusted_package_links_and_ambiguous_or_malformed_release_metadata() {
    let original = release("0.1.10");
    let mut cases = Vec::new();
    let mut value = original.clone();
    value["assets"][0]["browser_download_url"] =
        json!("https://example.invalid/synthetic.AppImage");
    cases.push(value);
    let mut value = original.clone();
    value["assets"][0]["size"] = json!(0);
    cases.push(value);
    let mut value = original.clone();
    value["assets"]
        .as_array_mut()
        .unwrap()
        .push(original["assets"][0].clone());
    cases.push(value);
    let mut value = original.clone();
    value["html_url"] = json!("https://example.invalid/synthetic-release");
    cases.push(value);
    let mut value = original;
    value["body"] = json!(42);
    cases.push(value);
    cases.push(json!({"tag_name": "v0.1.10"}));
    for value in cases {
        let (endpoint, worker) = fixture(json!([value]));
        assert_eq!(
            get(&endpoint, installed("0.1.9")).await,
            Err(CheckError::InvalidResponse)
        );
        received(worker).await;
    }
}

#[tokio::test]
async fn bounded_history_never_returns_a_false_up_to_date_result() {
    let page = serde_json::to_vec(&vec![release("0.1.9"); PAGE_SIZE]).unwrap();
    let (endpoint, worker) = serve(vec![(200, page, Duration::ZERO); MAX_PAGES]);
    assert_eq!(
        get(&endpoint, installed("0.1.9")).await,
        Err(CheckError::TooManyReleases)
    );
    assert_eq!(received(worker).await.len(), MAX_PAGES);
}

#[cfg(all(feature = "test-support", debug_assertions))]
#[test]
fn actual_runtime_discovers_authorizes_and_downloads_without_handoff_on_invalid_bytes() {
    const CHILD: &str = "IPTV_TEST_UPDATE_RUNTIME_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let worker = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        worker.block_on(runtime_update_journey());
        return;
    }
    let name = "Patina-9.8.7-android-tablet-tv.apk";
    let mut data = release("9.8.7");
    data["assets"] = json!([{
        "name": name, "size": 9,
        "browser_download_url": format!("{RELEASES}/download/v9.8.7/{name}")
    }]);
    let (endpoint, server) = serve(vec![
        (
            200,
            serde_json::to_vec(&json!([data])).unwrap(),
            Duration::ZERO,
        ),
        (200, b"synthetic".to_vec(), Duration::ZERO),
        (200, b"change-me".to_vec(), Duration::ZERO),
        (200, b"short".to_vec(), Duration::ZERO),
        (200, b"extra bytes".to_vec(), Duration::ZERO),
    ]);
    // Reuse this executable for isolated environment variables, rather than
    // mutating process-global environment while other tests are running.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "native::app_updates::github::tests::actual_runtime_discovers_authorizes_and_downloads_without_handoff_on_invalid_bytes", "--nocapture"])
        .env(CHILD, "1")
        .env("IPTV_TEST_UPDATE_API", &endpoint)
        .env("IPTV_TEST_UPDATE_DOWNLOAD_API", &endpoint)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 5);
    assert!(requests[0].starts_with("GET /releases?per_page=100&page=1 "));
    for request in &requests[1..] {
        assert!(request.starts_with("GET /releases "));
    }
}

#[cfg(all(feature = "test-support", debug_assertions))]
async fn runtime_update_journey() {
    use super::super::{
        UpdateResult,
        state::{Frequency, Trigger},
    };
    use sha2::{Digest, Sha256};

    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("profile");
    let make = || {
        crate::Runtime::new(crate::Ports::new(directory.clone(), |_| {
            Err(crate::model::StoreError::unavailable(
                "synthetic unavailable key",
            ))
        }))
    };
    let package = InstalledPackage {
        platform: "android",
        architecture: "arm64-v8a",
        format: Some("apk".into()),
        version: "9.8.6",
        android_code: Some(10_008_006),
        minimum: json!({"androidApi": 30}),
    };
    let owner = make();
    owner.start_services();
    owner.restore_app_update_state().await.unwrap();
    let token = owner
        .begin_app_update_check(1000, Trigger::Manual, Frequency::Never)
        .unwrap();
    let discovered = owner.check_github_release(package.clone()).await.unwrap();
    assert_eq!(discovered.status, ReleaseStatus::UpdateAvailable);
    assert_eq!(discovered.release.version, "9.8.7");
    assert!(discovered.release.notes.contains("Repair playback"));
    assert!(owner.finish_app_update_check(token, 1001, Ok(discovered.clone())));
    assert_eq!(owner.app_update_snapshot().cached, Some(discovered.clone()));

    // Authorize the exact package from signed synthetic repository metadata.
    // GitHub's release body and download URL never manufacture this asset.
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/updates");
    let mirror = temp.path().join("mirror");
    std::fs::create_dir_all(mirror.join("metadata")).unwrap();
    std::fs::create_dir_all(mirror.join("targets")).unwrap();
    for role in ["timestamp", "snapshot", "targets"] {
        let name = format!("1.{role}.json");
        let destination = if role == "timestamp" {
            "timestamp.json"
        } else {
            &name
        };
        std::fs::copy(
            fixtures.join(&name),
            mirror.join("metadata").join(destination),
        )
        .unwrap();
    }
    let index = std::fs::read(fixtures.join("index.json")).unwrap();
    std::fs::write(
        mirror
            .join("targets")
            .join(format!("{:x}.index.json", Sha256::digest(&index))),
        &index,
    )
    .unwrap();
    let root = std::fs::read(fixtures.join("1.root.json")).unwrap();
    let verified = super::super::verified_index(
        &temp.path().join("trust"),
        &root,
        &url::Url::from_directory_path(&mirror).unwrap(),
        tough::FilesystemTransport,
        package,
    )
    .await
    .unwrap();
    let UpdateResult::Available { version, asset, .. } = verified else {
        panic!("expected verified synthetic package");
    };
    assert_eq!(version, discovered.release.version);
    assert_eq!(
        Some(asset.url()),
        discovered
            .release
            .download
            .as_ref()
            .map(|download| download.url.as_str())
    );
    let downloads = temp.path().join("downloads");
    std::fs::create_dir(&downloads).unwrap();
    let handoff = owner
        .download_update(asset.clone(), downloads.clone())
        .await
        .unwrap();
    assert_eq!(std::fs::read(&handoff).unwrap(), b"synthetic");
    drop(handoff);
    for _ in 0..3 {
        assert!(
            owner
                .download_update(asset.clone(), downloads.clone())
                .await
                .is_none()
        );
        assert_eq!(
            std::fs::read_dir(&downloads).unwrap().count(),
            0,
            "invalid bytes must not leave a file eligible for installer handoff"
        );
    }
    owner.skip_app_update_version("9.8.7");
    owner.persist_app_update_state().await.unwrap();
    drop(owner);
    let restarted = make();
    restarted.start_services();
    restarted.restore_app_update_state().await.unwrap();
    assert_eq!(restarted.app_update_snapshot().last_success, Some(1001));
    assert_eq!(
        restarted.app_update_snapshot().skipped_version.as_deref(),
        Some("9.8.7")
    );
    assert!(
        restarted
            .begin_app_update_check(1002, Trigger::Startup, Frequency::Weekly)
            .is_none()
    );
}

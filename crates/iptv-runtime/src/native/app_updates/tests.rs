use super::*;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

fn installed() -> InstalledPackage {
    InstalledPackage {
        platform: "android",
        architecture: "arm64-v8a",
        format: Some("apk".into()),
        version: "9.8.6",
        android_code: Some(10_008_006),
        minimum: json!({"androidApi": 30}),
    }
}

#[tokio::test]
async fn verifies_targets_and_keeps_rotation_and_replay_state_across_checks() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/updates");
    let temp = tempfile::tempdir().unwrap();
    let base = Url::from_directory_path(temp.path()).unwrap();
    let metadata = temp.path().join("metadata");
    let targets = temp.path().join("targets");
    fs::create_dir(&metadata).unwrap();
    fs::create_dir(&targets).unwrap();
    let data = fs::read(fixture.join("index.json")).unwrap();
    let file = targets.join(format!("{:x}.index.json", Sha256::digest(&data)));
    fs::write(&file, &data).unwrap();
    let publish = |v| {
        for name in ["timestamp", "snapshot", "targets"] {
            let source = format!("{v}.{name}.json");
            let dest = if name == "timestamp" {
                "timestamp.json"
            } else {
                &source
            };
            fs::copy(fixture.join(source.as_str()), metadata.join(dest)).unwrap();
        }
    };
    let root = fs::read(fixture.join("1.root.json")).unwrap();
    let state = temp.path().join("client");
    let check = || {
        verified_index(
            &state,
            &root,
            &base,
            tough::FilesystemTransport,
            installed(),
        )
    };
    publish(1);
    assert!(matches!(
        check().await,
        Some(UpdateResult::Available { .. })
    ));
    fs::copy(fixture.join("2.root.json"), metadata.join("2.root.json")).unwrap();
    publish(2);
    fs::write(&file, b"changed").unwrap();
    assert!(check().await.is_none());
    fs::write(&file, &data).unwrap();
    // Subsequent checks start at the accepted root even after the mirror drops it.
    fs::remove_file(metadata.join("2.root.json")).unwrap();
    let trusted = fs::read(state.join("trusted.json")).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &trust::Session::open(&state, &root).unwrap().root
        )
        .unwrap(),
        serde_json::from_slice::<serde_json::Value>(
            &fs::read(fixture.join("2.root.json")).unwrap()
        )
        .unwrap()
    );
    publish(1);
    assert!(check().await.is_none());
    assert_eq!(fs::read(state.join("trusted.json")).unwrap(), trusted);
    publish(2);
    fs::write(&file, b"changed").unwrap();
    assert!(check().await.is_none());
    fs::write(&file, data).unwrap();
    assert!(matches!(
        check().await,
        Some(UpdateResult::Available { .. })
    ));
}

#[test]
fn respects_version_package_architecture_and_minimums() {
    let fixture = include_bytes!("../../../tests/fixtures/updates/index.json");
    let select = |p| selection::select(fixture, p).unwrap();
    let mut package = installed();
    package.android_code = Some(10_008_008);
    assert_eq!(select(package), UpdateResult::NoCompatiblePackage);
    for update in [
        |p: &mut InstalledPackage| p.minimum = json!({"androidApi":25}),
        |p: &mut InstalledPackage| p.architecture = "x86_64",
        |p: &mut InstalledPackage| p.format = None,
    ] {
        let mut package = installed();
        update(&mut package);
        assert_eq!(select(package), UpdateResult::NoCompatiblePackage);
    }
    let mut package = installed();
    package.version = "10.0.0";
    assert_eq!(select(package), UpdateResult::Current);
}

#[tokio::test]
async fn download_rejects_truncated_oversized_or_changed_bytes_and_cleans_up() {
    use iptv_import::MemoryBody;
    let dir = tempfile::tempdir().unwrap();
    let payload = b"synthetic";
    let digest = format!("{:x}", Sha256::digest(payload));
    for bytes in [b"change-me".as_slice(), b"short", b"extra bytes"] {
        assert!(
            download::receive(MemoryBody::new(bytes, 2), dir.path().into(), 9, &digest)
                .await
                .is_none()
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }
    let file = download::receive(MemoryBody::new(payload, 3), dir.path().into(), 9, &digest)
        .await
        .unwrap();
    assert_eq!(fs::read(&file).unwrap(), payload);
    drop(file);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

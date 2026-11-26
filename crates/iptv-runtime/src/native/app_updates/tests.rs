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
    // The new timestamp survives even when the mirror withholds its snapshot.
    fs::remove_file(metadata.join("2.snapshot.json")).unwrap();
    assert!(check().await.is_none());
    publish(1);
    assert!(check().await.is_none());
    publish(2);
    fs::write(&file, b"changed").unwrap();
    assert!(check().await.is_none());
    fs::write(&file, &data).unwrap();
    // Subsequent checks start at the accepted root even after the mirror drops it.
    fs::remove_file(metadata.join("2.root.json")).unwrap();
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
    let session = trust::Session::open(&state, &root).unwrap();
    let timestamp: serde_json::Value =
        serde_json::from_slice(&fs::read(session.work.path().join("timestamp.json")).unwrap())
            .unwrap();
    assert_eq!(timestamp["signed"]["version"], 2);
    drop(session);
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

#[test]
fn a_newer_desktop_requirement_needs_manual_confirmation() {
    let mut index: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../tests/fixtures/updates/index.json"))
            .unwrap();
    let entry = &mut index["packages"][0];
    entry["platform"] = json!("macos");
    entry["minimum"] = json!({"macos": "15.0"});
    entry["format"] = json!("dmg");
    entry["architectures"] = json!(["arm64"]);
    let mut package = installed();
    package.platform = "macos";
    package.format = Some("dmg".into());
    package.architecture = "arm64";
    package.minimum = json!({"macos":"14.0"});
    assert_eq!(
        selection::select(&serde_json::to_vec(&index).unwrap(), package),
        Some(UpdateResult::RequirementsUnconfirmed)
    );
}

#[test]
fn a_failed_refresh_cannot_pin_an_incorrect_future_clock() {
    let state = tempfile::tempdir().unwrap();
    let root = include_bytes!("../../../tests/fixtures/updates/1.root.json");
    let session = trust::Session::open(state.path(), root).unwrap();
    let accepted = br#""2026-01-01T00:00:00Z""#;
    let time = session.work.path().join("latest_known_time.json");
    fs::write(&time, accepted).unwrap();
    session.accept_clock().unwrap();
    session.commit().unwrap();
    fs::write(time, br#""2099-01-01T00:00:00Z""#).unwrap();
    session.commit().unwrap();
    drop(session);
    let restored = trust::Session::open(state.path(), root).unwrap();
    assert_eq!(
        fs::read(restored.work.path().join("latest_known_time.json")).unwrap(),
        accepted
    );
}

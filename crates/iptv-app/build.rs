//! Shell version, Windows resource stamping and iOS simulator entitlements.
mod build_version;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_version.rs");
    build_version::emit();
    println!("cargo:rustc-check-cfg=cfg(patina_airplay_probe)");
    println!("cargo:rerun-if-env-changed=PATINA_AIRPLAY_PROBE");
    let probe = std::env::var("PATINA_AIRPLAY_PROBE").as_deref() == Ok("1");
    if probe {
        println!("cargo:rustc-cfg=patina_airplay_probe");
        if matches!(
            std::env::var("CARGO_CFG_TARGET_OS").as_deref(),
            Ok("macos" | "ios")
        ) {
            let source = "src/host/native/apple_airplay.m";
            println!("cargo:rerun-if-changed={source}");
            cc::Build::new()
                .file(source)
                .flag("-fobjc-arc")
                .compile("patina_airplay_probe");
            println!("cargo:rustc-link-lib=framework=AVKit");
            println!("cargo:rustc-link-lib=framework=AVFoundation");
        }
    }
    println!("cargo:rerun-if-env-changed=PATINA_UPDATE_ROOT");
    println!("cargo:rerun-if-env-changed=PATINA_UPDATE_FEED");
    let root = std::env::var_os("PATINA_UPDATE_ROOT");
    let feed = std::env::var("PATINA_UPDATE_FEED").ok();
    assert_eq!(
        root.is_some(),
        feed.is_some(),
        "set both PATINA_UPDATE_ROOT and PATINA_UPDATE_FEED"
    );
    if let Some(feed) = feed {
        assert!(
            feed.starts_with("https://") && feed.ends_with('/'),
            "the update repository requires an HTTPS directory URL"
        );
    }
    let bytes = root.map_or_else(Vec::new, |path| {
        let path = std::path::PathBuf::from(path)
            .canonicalize()
            .expect("resolve public TUF root");
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes = std::fs::read(path).expect("read public TUF root");
        let root: serde_json::Value =
            serde_json::from_slice(&bytes).expect("parse public TUF root JSON");
        assert!(
            root["signed"]["_type"] == "root"
                && root["signed"]["version"].as_u64().is_some_and(|v| v > 0)
                && root["signatures"].as_array().is_some_and(|s| !s.is_empty()),
            "expected signed, versioned public TUF root"
        );
        bytes
    });
    let out =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo output directory"));
    std::fs::write(out.join("update-root.json"), bytes).expect("embed update trust root");
    if let Ok("windows") = std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        println!("cargo:rerun-if-changed=windows.rc");
        println!("cargo:rerun-if-changed=icon.ico");
        embed_resource::compile_for("windows.rc", ["iptv-app"], embed_resource::NONE)
            .manifest_required()
            .expect("compile Windows app icon resource");
    }
    let os = std::env::var("CARGO_CFG_TARGET_OS");
    let abi = std::env::var("CARGO_CFG_TARGET_ABI");
    if let (Ok("ios"), Ok("sim")) = (os.as_deref(), abi.as_deref()) {
        let manifest = if probe {
            "../../tools/apple-airplay-probe/nsh.json"
        } else {
            "nsh.json"
        };
        simulator_entitlements(manifest);
    }
}

/// Writes the iOS simulator's entitlements for `src/host/native/simulator_entitlements.rs`
/// to link: the app's identity and its own Keychain access group, both the
/// bundle id in `nsh.json`, as Xcode gives a simulator build.
fn simulator_entitlements(manifest: &str) {
    println!("cargo:rerun-if-changed={manifest}");
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(manifest).expect("read nsh.json"))
            .expect("parse nsh.json");
    let id = config["id"].as_str().expect("nsh.json has an `id`");
    assert!(
        !id.is_empty()
            && id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-'),
        "nsh.json `id` is not a bundle identifier: {id:?}"
    );
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>application-identifier</key>
	<string>{id}</string>
	<key>keychain-access-groups</key>
	<array>
		<string>{id}</string>
	</array>
</dict>
</plist>
"#
    );
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(out.join("simulator.entitlements"), plist)
        .expect("write the simulator entitlements");
}

//! Shell version, Windows resource stamping and iOS simulator entitlements.
mod build_version;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_version.rs");
    build_version::emit();
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
        simulator_entitlements();
    }
}

/// Writes the iOS simulator's entitlements for `src/host/native/simulator_entitlements.rs`
/// to link: the app's identity and its own Keychain access group, both the
/// bundle id in `nsh.json`, as Xcode gives a simulator build.
fn simulator_entitlements() {
    println!("cargo:rerun-if-changed=nsh.json");
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read("nsh.json").expect("read nsh.json"))
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

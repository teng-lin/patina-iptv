use std::path::Path;

fn check_sources(directory: &Path) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            check_sources(&path);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let code = std::fs::read_to_string(&path).unwrap();
            for forbidden in ["nsh_", "iptv_app", "iptv_player_mpv", "crate::Ui"] {
                assert!(
                    !code.contains(forbidden),
                    "{} uses {forbidden}",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn runtime_has_no_shell_or_toolkit_dependencies() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    for forbidden in ["nsh-", "iptv-app", "iptv-player-mpv", "jni", "keyring"] {
        assert!(
            !manifest.contains(forbidden),
            "runtime declares {forbidden}"
        );
    }
    check_sources(&root.join("src"));
}

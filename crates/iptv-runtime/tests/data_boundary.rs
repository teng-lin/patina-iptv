use std::path::Path;

fn check_shell(directory: &Path) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            check_shell(&path);
        } else if path.extension().is_some_and(|extension| extension == "rs")
            && path.file_name().unwrap() != "runtime_events.rs"
        {
            let source = std::fs::read_to_string(&path).unwrap();
            assert!(
                !source.contains("RuntimeEvent::"),
                "{} produces or handles runtime events outside the presentation entry point",
                path.display()
            );
        }
    }
}

#[test]
fn shell_only_handles_runtime_events_at_its_presentation_entry_point() {
    check_shell(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../iptv-app/src"));
}

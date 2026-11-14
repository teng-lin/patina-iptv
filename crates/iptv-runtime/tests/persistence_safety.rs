use iptv_runtime::{Ports, Runtime};
use std::{rc::Rc, time::Duration};

fn owner(path: &std::path::Path) -> Rc<Runtime> {
    Rc::new(Runtime::new(Ports::new(path.to_owned(), |_| {
        Ok(iptv_runtime::model::DatabaseKey::from_bytes([0x67; 32]))
    })))
}

#[test]
fn failed_unreadable_settings_rename_prevents_overwriting_the_original() {
    let root = tempfile::tempdir().unwrap();
    let original = b"generated invalid settings";
    std::fs::write(root.path().join("settings.json"), original).unwrap();
    std::fs::create_dir(root.path().join("settings.unreadable.json")).unwrap();
    let runtime = owner(root.path());
    runtime.start_services();
    runtime.load_settings();
    assert!(
        runtime
            .set_setting("catchup.forward_step", iptv_settings::Value::secs(60))
            .is_err()
    );
    assert!(runtime.flush_files(Duration::from_secs(2)));
    assert!(runtime.drain_services());
    assert_eq!(
        std::fs::read(root.path().join("settings.json")).unwrap(),
        original
    );
}

#[test]
fn combined_resume_is_included_in_the_host_file_flush() {
    let root = tempfile::tempdir().unwrap();
    let runtime = owner(root.path());
    runtime.start_services();
    runtime.save_combined_channel("generated-combined-channel");
    assert_eq!(
        runtime.load_combined_channel().as_deref(),
        Some("generated-combined-channel")
    );
    assert!(runtime.flush_files(Duration::from_secs(2)));
    assert!(runtime.drain_services());
    assert_eq!(
        std::fs::read_to_string(root.path().join("last_channel_combined")).unwrap(),
        "generated-combined-channel"
    );
}

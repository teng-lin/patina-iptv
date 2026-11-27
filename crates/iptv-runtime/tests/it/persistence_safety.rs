use super::support::block_on;
use iptv_runtime::{
    Ports, Runtime, commands,
    file_host::{FileHost, Stored},
};
use std::{
    path::Path,
    rc::Rc,
    sync::{Arc, Mutex},
    thread::{self, ThreadId},
    time::Duration,
};

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

#[test]
fn combined_resume_prefers_the_host_file_and_reads_it_off_the_owner_thread() {
    #[derive(Default)]
    struct Host(Mutex<Vec<ThreadId>>);

    impl FileHost for Host {
        fn read(&self, path: &Path) -> Option<Vec<u8>> {
            if path.file_name()? != "last_channel_combined" {
                return None;
            }
            self.0.lock().unwrap().push(thread::current().id());
            Some(b"generated-file-channel".to_vec())
        }

        fn write(&self, _: &Path, _: Option<Vec<u8>>) -> Stored {
            Box::pin(std::future::ready(true))
        }
    }

    let root = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::default());
    let mut ports = Ports::new(root.path().to_owned(), |_| {
        Ok(iptv_runtime::model::DatabaseKey::from_bytes([0x67; 32]))
    });
    ports.file_host = Some(host.clone());
    let runtime = Runtime::new(ports);
    runtime.start_services();
    block_on(commands::remember_combined_channel(
        &runtime,
        "generated-stored-channel".into(),
    ))
    .unwrap();

    assert_eq!(
        block_on(commands::combined_resume_key(&runtime))
            .unwrap()
            .as_deref(),
        Some("generated-file-channel")
    );
    let reads = host.0.lock().unwrap();
    assert_eq!(reads.len(), 1);
    assert_ne!(reads[0], thread::current().id());
    drop(reads);
    assert!(runtime.drain_services());
}

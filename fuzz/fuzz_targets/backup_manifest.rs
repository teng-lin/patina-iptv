#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 65536 {
        return;
    }
    let _ = iptv_import::backup_manifest::parse(data);
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = iptv_import::backup_manifest::allowed(s);
    }
});

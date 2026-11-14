#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 65536 {
        return;
    }
    let _ = iptv_import::import_m3u_text(
        data,
        &mut iptv_import::NoProgress,
        &iptv_import::CancelToken::new(),
    );
    let _ = iptv_epg::parse_xmltv(data, Default::default());
});

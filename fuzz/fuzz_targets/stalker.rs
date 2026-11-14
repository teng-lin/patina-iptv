#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 65536 {
        return;
    }
    let response = iptv_providers::HttpResponse::new(200, data);
    let _ = iptv_providers::stalker::parse_channel_page(&response);
    let _ = iptv_providers::stalker::parse_vod_page(&response);
});

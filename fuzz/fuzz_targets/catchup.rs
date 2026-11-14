#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 65536 {
        return;
    }
    let text = String::from_utf8_lossy(data);
    let now = data
        .get(..8)
        .map(|b| i64::from_le_bytes(b.try_into().unwrap()))
        .unwrap_or(0);
    let _ = iptv_providers::m3u::expand_live_url(&text, now, 0);
});

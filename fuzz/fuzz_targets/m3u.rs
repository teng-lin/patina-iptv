#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 65536 {
        return;
    }
    let mut p = iptv_providers::m3u::M3uParser::new();
    for c in data.chunks(1 + data.len() % 97) {
        p.feed(c);
    }
    let _ = p.finish();
});

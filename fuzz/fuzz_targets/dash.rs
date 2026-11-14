#![no_main]
use iptv_net::playback::fuzzing::rewrite_dash;
use libfuzzer_sys::fuzz_target;

const PREFIX: &str = "http://127.0.0.1:1/0123456789abcdef/";
const BASES: [&str; 4] = [
    "https://media.example.test/live/main.m3u8",
    "http://media.example.test:8080/a/b/main.mpd?token=synthetic",
    "http://media.example.test",
    "https://[2001:db8::1]:8443/v/",
];

fuzz_target!(|data: &[u8]| {
    if data.len() > 65536 {
        return;
    }
    for base in BASES {
        let _ = rewrite_dash(data, base, PREFIX);
    }
});

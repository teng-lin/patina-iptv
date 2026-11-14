#![no_main]
use iptv_net::playback::fuzzing::rewrite_hls;
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
        let clean = data.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(data);
        let Some(out) = rewrite_hls(clean, base, PREFIX) else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(&out) else {
            panic!("rewritten playlist is not text");
        };
        for line in text.split(['\n', '\r']) {
            for line in [line.trim(), line.trim_matches(|c: char| c <= ' ')] {
                if !line.is_empty() && !line.starts_with('#') {
                    assert!(line.starts_with(PREFIX), "URI not rewritten to prefix: {line}");
                }
            }
        }
    }
});

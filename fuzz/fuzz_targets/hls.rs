#![no_main]
//! HLS relay invariant, including FFmpeg attribute parsing and Media3 URI search.
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
        hls(data, base);
    }
});

fn hls(data: &[u8], base: &str) {
    // The relay drops a byte order mark before rewriting.
    let data = data.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(data);
    let Some(out) = rewrite_hls(data, base, PREFIX) else {
        return;
    };
    let text = std::str::from_utf8(&out).expect("a rewritten playlist is text");
    for line in text.split(['\n', '\r']) {
        // As Rust and as Java trim a line: a player may do either.
        for line in [line.trim(), line.trim_matches(|c: char| c <= ' ')] {
            if line.is_empty() {
                continue;
            }
            if !line.starts_with('#') {
                assert!(line.starts_with(PREFIX), "URI line off the relay: {line:?}");
                continue;
            }
            check_attribute_uris(line);
            let mut rest = line;
            while let Some(at) = rest.find("URI=\"") {
                let value = &rest[at + 5..];
                assert!(
                    value.starts_with(PREFIX),
                    "URI attribute off the relay: {line:?}"
                );
                let end = value
                    .find('"')
                    .expect("a rewritten URI attribute is closed");
                rest = &value[end + 1..];
            }
        }
    }
}

// Read attributes as a permissive player does, independently of the
// rewriter: commas delimit fields, quotes may escape their next byte,
// and an unquoted value ends at whitespace as well as a comma.
fn check_attribute_uris(line: &str) {
    let Some((_, mut rest)) = line.split_once(':') else {
        return;
    };
    while !rest.is_empty() {
        rest = rest.trim_start_matches([',', ' ', '\t']);
        let Some((name, tail)) = rest.split_once('=') else {
            break;
        };
        let mut value = String::new();
        let mut consumed = 0;
        if let Some(quoted) = tail.strip_prefix('"') {
            consumed = 1;
            let mut escaped = false;
            for ch in quoted.chars() {
                consumed += ch.len_utf8();
                if escaped {
                    value.push(ch);
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    break;
                } else {
                    value.push(ch);
                }
            }
        } else {
            for ch in tail.chars() {
                if ch == ',' || ch.is_ascii_whitespace() {
                    break;
                }
                consumed += ch.len_utf8();
                value.push(ch);
            }
        }
        if name.eq_ignore_ascii_case("URI") || name.ends_with("-URI") {
            assert!(
                value.starts_with(PREFIX),
                "player URI attribute off relay: {line:?}"
            );
        }
        rest = &tail[consumed..];
        // A player advances to the next comma after a value.
        let Some((_, next)) = rest.split_once(',') else {
            break;
        };
        rest = next;
    }
}

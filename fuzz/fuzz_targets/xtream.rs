#![no_main]
use iptv_providers::xtream::{ListKind, XtreamListParser};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 65536 {
        return;
    }
    for kind in [ListKind::Live, ListKind::Vod, ListKind::Series] {
        let mut p = XtreamListParser::new(kind, Default::default());
        for c in data.chunks(1 + data.len() % 97) {
            if p.feed(c, &mut |_| {}).is_err() {
                break;
            }
        }
        let _ = p.finish(&mut |_| {});
    }
});

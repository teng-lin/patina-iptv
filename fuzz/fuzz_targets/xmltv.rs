#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 65536 {
        return;
    }
    let mut p = iptv_epg::XmltvParser::new(Default::default());
    for c in data.chunks(1 + data.len() % 97) {
        if p.feed(c, &mut |_| {}).is_err() {
            return;
        }
    }
    let _ = p.finish(&mut |_| {});
});

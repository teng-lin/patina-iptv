//! Allocation regressions share a link, with a fresh process for each case.

mod allocations;
mod catalog_stream_memory;
mod epg_memory;
mod epg_stream_memory;

#[path = "../common/mod.rs"]
mod common;
#[path = "../epg_synth/mod.rs"]
mod epg_synth;
#[path = "../initialization_support/mod.rs"]
mod initialization_support;
#[path = "../staged/mod.rs"]
mod staged;
#[path = "../stream_support/mod.rs"]
mod stream_support;

#[global_allocator]
static ALLOC: allocations::Counting = allocations::Counting;

// Bound total fixture memory as before, when Cargo ran separate binaries in order.
static MEASUREMENT: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn isolated(test: &str, measure: impl FnOnce()) {
    const CHILD: &str = "IPTV_STORE_MEMORY_CHILD";
    if std::env::var_os(CHILD).is_some() {
        measure();
        return;
    }
    let _measurement = MEASUREMENT
        .lock()
        .expect("memory measurement lock poisoned");
    let output = initialization_support::child(test, CHILD, std::time::Duration::from_secs(600));
    assert!(
        output.status.success(),
        "memory case {test} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
}
